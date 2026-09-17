//! SQLite hook runtime for a dedicated macOS event database worker.
//! This is not an OS sandbox; the parent owns authority and confirmed process exit.
use crate::skill_event_files::{open_event_file, EventFile, OpenFailure, OpenMode};
use cap_std::fs::Dir;
use rusqlite::{ffi, Connection, OpenFlags};
use std::{
    collections::HashMap,
    ffi::{CStr, CString},
    os::{
        fd::{AsRawFd, IntoRawFd},
        unix::fs::MetadataExt,
    },
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Copy)]
struct FileIdentity {
    role: EventFile,
    device: u64,
    inode: u64,
}

struct Mapping {
    length: usize,
    charged_bytes: usize,
}
const MAX_MAPPINGS: usize = 16;
const MAX_MAPPED_BYTES: usize = 64 * 1024 * 1024;

type NativeOpen = unsafe extern "C" fn(
    *mut ffi::sqlite3_vfs,
    *const libc::c_char,
    *mut ffi::sqlite3_file,
    i32,
    *mut i32,
) -> i32;
static NATIVE_OPEN: OnceLock<NativeOpen> = OnceLock::new();
static SELECTED_VFS: OnceLock<&'static CStr> = OnceLock::new();
struct IoMethods {
    methods: Box<ffi::sqlite3_io_methods>,
    original: usize,
}
struct State {
    io_methods: HashMap<usize, IoMethods>,
    sync_calls: usize,
    #[cfg(test)]
    sync_failures: usize,
    #[cfg(test)]
    fail_sync: bool,
    directory: Dir,
    descriptors: HashMap<i32, Option<FileIdentity>>,
    denied: bool,
    opened_roles: [usize; 4],
    descriptor_calls: [usize; 3],
    read_calls: [usize; 2],
    mappings: HashMap<usize, Mapping>,
    mapped_bytes: usize,
    peak_mapped_bytes: usize,
    mapping_calls: [usize; 2],
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn fail(code: i32) -> i32 {
    // macOS provides a writable errno slot for the calling thread.
    unsafe {
        *libc::__error() = code;
    }
    -1
}
unsafe fn role(path: *const libc::c_char) -> Option<EventFile> {
    if path.is_null() {
        return None;
    }
    // SQLite supplies a valid NUL-terminated filename for each path callback.
    match unsafe { CStr::from_ptr(path) }.to_bytes() {
        b"/skill-studio-event/events.sqlite3" => Some(EventFile::Database),
        b"/skill-studio-event/events.sqlite3-wal" => Some(EventFile::Wal),
        b"/skill-studio-event/events.sqlite3-shm" => Some(EventFile::SharedMemory),
        b"/skill-studio-event/events.sqlite3-journal" => Some(EventFile::Journal),
        _ => None,
    }
}
fn deny() -> i32 {
    STATE.get().unwrap().lock().unwrap().denied = true;
    fail(libc::EACCES)
}
unsafe extern "C" fn open(path: *const libc::c_char, flags: i32, _mode: i32) -> i32 {
    let Some(role) = (unsafe { role(path) }) else {
        return deny();
    };
    if flags & libc::O_ACCMODE != libc::O_RDWR || flags & (libc::O_TRUNC | libc::O_APPEND) != 0 {
        return deny();
    }
    let mut state = STATE.get().unwrap().lock().unwrap();
    if state.descriptors.len() >= 16 {
        state.denied = true;
        return fail(libc::EMFILE);
    }
    let mode = if flags & libc::O_EXCL != 0 {
        OpenMode::CreateNew
    } else {
        match state.directory.symlink_metadata(role.name()) {
            Ok(_) => OpenMode::Existing,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && flags & libc::O_CREAT != 0 =>
            {
                OpenMode::CreateNew
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return fail(libc::ENOENT)
            }
            Err(error) => return fail(error.raw_os_error().unwrap_or(libc::EIO)),
        }
    };
    match open_event_file(&state.directory, role, mode) {
        Ok(file) => {
            let metadata = match file.metadata() {
                Ok(metadata) => metadata,
                Err(error) => return fail(error.raw_os_error().unwrap_or(libc::EIO)),
            };
            let identity = FileIdentity {
                role,
                device: metadata.dev(),
                inode: metadata.ino(),
            };
            let descriptor = file.into_raw_fd();
            state.descriptors.insert(descriptor, Some(identity));
            state.opened_roles[role as usize] += 1;
            descriptor
        }
        Err(OpenFailure::NotCreated(error) | OpenFailure::CreationAttempted(error)) => {
            state.denied = true;
            fail(error.raw_os_error().unwrap_or(libc::EACCES))
        }
    }
}
fn file_metadata(descriptor: i32) -> Option<libc::stat> {
    use cap_std::fs::MetadataExt as _;
    let state = STATE.get().unwrap().lock().unwrap();
    let Some(Some(identity)) = state.descriptors.get(&descriptor) else {
        return None;
    };
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // fstat observes the registered descriptor without duplicating or closing it.
    if unsafe { libc::fstat(descriptor, metadata.as_mut_ptr()) } != 0 {
        return None;
    }
    let metadata = unsafe { metadata.assume_init() };
    let Ok(entry) = state.directory.symlink_metadata(identity.role.name()) else {
        return None;
    };
    (metadata.st_mode & libc::S_IFMT == libc::S_IFREG
        && metadata.st_nlink == 1
        && metadata.st_dev as u64 == identity.device
        && metadata.st_ino == identity.inode
        && entry.is_file()
        && entry.nlink() == 1
        && entry.dev() == identity.device
        && entry.ino() == identity.inode)
        .then_some(metadata)
}
fn valid_file(descriptor: i32) -> bool {
    file_metadata(descriptor).is_some()
}
unsafe extern "C" fn fstat(descriptor: i32, output: *mut libc::stat) -> i32 {
    if output.is_null() {
        return fail(libc::EINVAL);
    }
    let Some(metadata) = file_metadata(descriptor) else {
        return deny();
    };
    unsafe { output.write(metadata) };
    0
}
fn permission_metadata(descriptor: i32) -> Option<(libc::stat, libc::stat)> {
    let target = file_metadata(descriptor)?;
    let database = {
        let state = STATE.get().unwrap().lock().unwrap();
        let identity = state.descriptors.get(&descriptor)?.as_ref()?;
        if matches!(identity.role, EventFile::Database) {
            return None;
        }
        state.descriptors.iter().find_map(|(fd, identity)| {
            identity
                .as_ref()
                .filter(|identity| matches!(identity.role, EventFile::Database))
                .map(|_| *fd)
        })?
    };
    Some((target, file_metadata(database)?))
}
unsafe extern "C" fn fchmod(descriptor: i32, mode: libc::mode_t) -> i32 {
    let Some((target, database)) = permission_metadata(descriptor) else {
        return deny();
    };
    if target.st_size != 0 || mode != database.st_mode & 0o777 {
        return deny();
    }
    unsafe { libc::fchmod(descriptor, mode) }
}
unsafe extern "C" fn fchown(descriptor: i32, uid: libc::uid_t, gid: libc::gid_t) -> i32 {
    let Some((target, database)) = permission_metadata(descriptor) else {
        return deny();
    };
    if uid != database.st_uid || gid != database.st_gid {
        return deny();
    }
    if target.st_size != 0 && (target.st_uid != uid || target.st_gid != gid) {
        return deny();
    }
    unsafe { libc::fchown(descriptor, uid, gid) }
}
unsafe extern "C" {
    fn skill_studio_event_set_fd_authorizer(
        authorize: unsafe extern "C" fn(i32, i32, *const libc::flock, i32) -> i32,
    );
    fn skill_studio_event_fcntl(descriptor: i32, command: i32, ...) -> i32;
}
fn valid_directory_descriptor(descriptor: i32) -> bool {
    use cap_std::fs::MetadataExt as _;
    let state = STATE.get().unwrap().lock().unwrap();
    if !matches!(state.descriptors.get(&descriptor), Some(None)) {
        return false;
    }
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(descriptor, metadata.as_mut_ptr()) } != 0 {
        return false;
    }
    let metadata = unsafe { metadata.assume_init() };
    let Ok(root) = state.directory.dir_metadata() else {
        return false;
    };
    metadata.st_mode & libc::S_IFMT == libc::S_IFDIR
        && metadata.st_dev as u64 == root.dev()
        && metadata.st_ino == root.ino()
}
fn valid_lock_range(role: EventFile, command: i32, lock: &libc::flock) -> bool {
    if lock.l_whence != libc::SEEK_SET as i16
        || !matches!(lock.l_type, libc::F_RDLCK | libc::F_WRLCK | libc::F_UNLCK)
        || (command == libc::F_GETLK && lock.l_type == libc::F_UNLCK)
    {
        return false;
    }
    if matches!(role, EventFile::Database) && lock.l_start == 0 {
        return (command == libc::F_GETLK && lock.l_len == 1)
            || (command != libc::F_GETLK && lock.l_type == libc::F_UNLCK && lock.l_len == 0);
    }
    if lock.l_len <= 0 {
        return false;
    }
    let Some(end) = lock.l_start.checked_add(lock.l_len) else {
        return false;
    };
    match role {
        EventFile::Database => lock.l_start >= 0x4000_0000 && end <= 0x4000_0200,
        EventFile::SharedMemory => {
            let base = (22 + i64::from(ffi::SQLITE_SHM_NLOCK)) * 4;
            let deadman = base + i64::from(ffi::SQLITE_SHM_NLOCK);
            (lock.l_start >= base && end <= deadman) || (lock.l_start == deadman && lock.l_len == 1)
        }
        _ => false,
    }
}
unsafe extern "C" fn authorize_fcntl(
    descriptor: i32,
    command: i32,
    lock: *const libc::flock,
    _flags: i32,
) -> i32 {
    let allowed = match command {
        libc::F_GETLK | libc::F_SETLK | libc::F_SETLKW => {
            let role = STATE
                .get()
                .unwrap()
                .lock()
                .unwrap()
                .descriptors
                .get(&descriptor)
                .and_then(Option::as_ref)
                .map(|identity| identity.role);
            // The C bridge receives a valid SQLite flock pointer and checks null first.
            let lock = unsafe { lock.as_ref() };
            valid_file(descriptor)
                && role
                    .zip(lock)
                    .is_some_and(|(role, lock)| valid_lock_range(role, command, lock))
        }
        libc::F_GETFD | libc::F_SETFD | libc::F_FULLFSYNC => {
            valid_file(descriptor) || valid_directory_descriptor(descriptor)
        }
        _ => false,
    };
    if allowed {
        1
    } else {
        deny();
        0
    }
}
unsafe extern "C" fn mkdir(_path: *const libc::c_char, _mode: libc::mode_t) -> i32 {
    deny()
}
unsafe extern "C" fn rmdir(_path: *const libc::c_char) -> i32 {
    deny()
}
unsafe extern "C" fn read(descriptor: i32, bytes: *mut libc::c_void, count: usize) -> isize {
    if !valid_file(descriptor) {
        return deny() as isize;
    }
    if (bytes.is_null() && count != 0) || count > isize::MAX as usize {
        return fail(libc::EINVAL) as isize;
    }
    STATE.get().unwrap().lock().unwrap().read_calls[0] += 1;
    // SQLite owns this writable output buffer; rejected descriptors never reach it.
    unsafe { libc::read(descriptor, bytes, count) }
}
unsafe extern "C" fn pread(
    descriptor: i32,
    bytes: *mut libc::c_void,
    count: usize,
    offset: libc::off_t,
) -> isize {
    if !valid_file(descriptor) {
        return deny() as isize;
    }
    if (bytes.is_null() && count != 0)
        || count > isize::MAX as usize
        || offset < 0
        || offset.checked_add(count as i64).is_none()
    {
        return fail(libc::EINVAL) as isize;
    }
    STATE.get().unwrap().lock().unwrap().read_calls[1] += 1;
    unsafe { libc::pread(descriptor, bytes, count, offset) }
}

#[cfg(test)]
fn assert_reads_refused(descriptor: i32) {
    let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
    metadata.st_ino = 12345;
    metadata.st_size = 67890;
    assert_eq!(unsafe { fstat(descriptor, &mut metadata) }, -1);
    assert_eq!(metadata.st_ino, 12345);
    assert_eq!(metadata.st_size, 67890);
    assert_eq!(
        unsafe {
            mmap(
                std::ptr::null_mut(),
                1,
                libc::PROT_READ,
                libc::MAP_SHARED,
                descriptor,
                0,
            )
        },
        libc::MAP_FAILED
    );
    let mut output = [0x5au8; 16];
    // These calls deliberately use stale or foreign descriptors with a live buffer.
    unsafe {
        assert_eq!(
            read(descriptor, output.as_mut_ptr().cast(), output.len()),
            -1
        );
        assert_eq!(output, [0x5a; 16]);
        assert_eq!(
            pread(descriptor, output.as_mut_ptr().cast(), output.len(), 0),
            -1
        );
        assert_eq!(output, [0x5a; 16]);
    }
}

unsafe extern "C" fn write(descriptor: i32, bytes: *const libc::c_void, count: usize) -> isize {
    if !valid_file(descriptor) {
        return deny() as isize;
    }
    if (bytes.is_null() && count != 0) || count > isize::MAX as usize {
        return fail(libc::EINVAL) as isize;
    }
    STATE.get().unwrap().lock().unwrap().descriptor_calls[0] += 1;
    // SQLite owns the supplied buffer; file authority was checked without opening a second handle.
    unsafe { libc::write(descriptor, bytes, count) }
}
unsafe extern "C" fn pwrite(
    descriptor: i32,
    bytes: *const libc::c_void,
    count: usize,
    offset: libc::off_t,
) -> isize {
    if !valid_file(descriptor) {
        return deny() as isize;
    }
    if (bytes.is_null() && count != 0)
        || count > isize::MAX as usize
        || offset < 0
        || offset.checked_add(count as i64).is_none()
    {
        return fail(libc::EINVAL) as isize;
    }
    STATE.get().unwrap().lock().unwrap().descriptor_calls[1] += 1;
    unsafe { libc::pwrite(descriptor, bytes, count, offset) }
}
unsafe extern "C" fn truncate(descriptor: i32, length: libc::off_t) -> i32 {
    if !valid_file(descriptor) {
        return deny();
    }
    if length < 0 {
        return fail(libc::EINVAL);
    }
    STATE.get().unwrap().lock().unwrap().descriptor_calls[2] += 1;
    unsafe { libc::ftruncate(descriptor, length) }
}
unsafe extern "C" fn mmap(
    address: *mut libc::c_void,
    length: usize,
    protection: i32,
    flags: i32,
    descriptor: i32,
    offset: libc::off_t,
) -> *mut libc::c_void {
    if !valid_file(descriptor) {
        deny();
        return libc::MAP_FAILED;
    }
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page <= 0
        || length == 0
        || length > isize::MAX as usize
        || offset < 0
        || offset % page != 0
        || flags != libc::MAP_SHARED
    {
        deny();
        return libc::MAP_FAILED;
    }
    let Some(charged) = length
        .checked_add(page as usize - 1)
        .map(|value| value / page as usize * page as usize)
    else {
        deny();
        return libc::MAP_FAILED;
    };
    let mut state = STATE.get().unwrap().lock().unwrap();
    let permitted = match state.descriptors.get(&descriptor) {
        Some(Some(identity)) => match identity.role {
            EventFile::Database => protection == libc::PROT_READ,
            EventFile::SharedMemory => {
                protection == libc::PROT_READ || protection == (libc::PROT_READ | libc::PROT_WRITE)
            }
            _ => false,
        },
        _ => false,
    };
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    let inside_file = if unsafe { libc::fstat(descriptor, metadata.as_mut_ptr()) } == 0 {
        let metadata = unsafe { metadata.assume_init() };
        offset
            .checked_add(length as i64)
            .is_some_and(|end| end <= metadata.st_size)
    } else {
        false
    };
    if !permitted
        || !inside_file
        || state.mappings.len() >= MAX_MAPPINGS
        || charged > MAX_MAPPED_BYTES.saturating_sub(state.mapped_bytes)
    {
        state.denied = true;
        fail(libc::EACCES);
        return libc::MAP_FAILED;
    }
    state.mapping_calls[0] += 1;
    // MAP_FIXED is refused; the kernel chooses an unoccupied region for this approved file.
    let mapped = unsafe { libc::mmap(address, length, protection, flags, descriptor, offset) };
    if mapped != libc::MAP_FAILED {
        state.mappings.insert(
            mapped as usize,
            Mapping {
                length,
                charged_bytes: charged,
            },
        );
        state.mapped_bytes += charged;
        state.peak_mapped_bytes = state.peak_mapped_bytes.max(state.mapped_bytes);
    }
    mapped
}
unsafe extern "C" fn munmap(address: *mut libc::c_void, length: usize) -> i32 {
    let mut state = STATE.get().unwrap().lock().unwrap();
    let Some(mapping) = state.mappings.get(&(address as usize)) else {
        state.denied = true;
        return fail(libc::EACCES);
    };
    if mapping.length != length {
        state.denied = true;
        return fail(libc::EACCES);
    }
    let charged = mapping.charged_bytes;
    state.mapping_calls[1] += 1;
    // Exact registered regions may be released even after file/root drift.
    let result = unsafe { libc::munmap(address, length) };
    if result == 0 {
        state.mappings.remove(&(address as usize));
        state.mapped_bytes -= charged;
    } else {
        state.denied = true;
    }
    result
}

unsafe extern "C" fn close(descriptor: i32) -> i32 {
    let mut state = STATE.get().unwrap().lock().unwrap();
    use cap_std::fs::MetadataExt as _;
    let expected = match state.descriptors.get(&descriptor) {
        Some(Some(identity)) => Some((identity.device, identity.inode)),
        Some(None) => state
            .directory
            .dir_metadata()
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino())),
        None => None,
    };
    let Some(expected) = expected else {
        state.denied = true;
        return fail(libc::EBADF);
    };
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    let current = if unsafe { libc::fstat(descriptor, metadata.as_mut_ptr()) } == 0 {
        let metadata = unsafe { metadata.assume_init() };
        Some((metadata.st_dev as u64, metadata.st_ino))
    } else {
        None
    };
    if Some(expected) != current {
        state.denied = true;
        return fail(libc::EBADF);
    }
    // SQLite controls close timing. Retain uncertainty if the native close fails.
    let result = unsafe { libc::close(descriptor) };
    if result == 0 {
        state.descriptors.remove(&descriptor);
    } else {
        state.denied = true;
    }
    result
}
unsafe extern "C" fn stat(path: *const libc::c_char, output: *mut libc::stat) -> i32 {
    let Some(role) = (unsafe { role(path) }) else {
        return deny();
    };
    if output.is_null() {
        return deny();
    }
    let state = STATE.get().unwrap().lock().unwrap();
    let name = CString::new(role.name()).unwrap();
    // Fixed component under the retained directory; SQLite owns output storage.
    let result = unsafe {
        libc::fstatat(
            state.directory.as_raw_fd(),
            name.as_ptr(),
            output,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0
        && unsafe { (*output).st_mode & libc::S_IFMT != libc::S_IFREG || (*output).st_nlink != 1 }
    {
        drop(state);
        return deny();
    }
    result
}
unsafe extern "C" fn access(path: *const libc::c_char, _mode: i32) -> i32 {
    let mut metadata = std::mem::MaybeUninit::uninit();
    unsafe { stat(path, metadata.as_mut_ptr()) }
}
unsafe extern "C" fn unlink(path: *const libc::c_char) -> i32 {
    let Some(role) = (unsafe { role(path) }) else {
        return deny();
    };
    let mut metadata = std::mem::MaybeUninit::uninit();
    if unsafe { stat(path, metadata.as_mut_ptr()) } != 0 {
        return -1;
    }
    match STATE
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .directory
        .remove_file(role.name())
    {
        Ok(()) => 0,
        Err(error) => fail(error.raw_os_error().unwrap_or(libc::EIO)),
    }
}
unsafe extern "C" fn open_directory(path: *const libc::c_char, output: *mut i32) -> i32 {
    if output.is_null() || unsafe { role(path) }.is_none() {
        deny();
        return ffi::SQLITE_CANTOPEN;
    }
    let mut state = STATE.get().unwrap().lock().unwrap();
    if state.descriptors.len() >= 16 {
        return ffi::SQLITE_CANTOPEN;
    }
    match state.directory.open(".") {
        Ok(directory) => {
            let descriptor = directory.into_std().into_raw_fd();
            state.descriptors.insert(descriptor, None);
            // SQLite supplies one writable descriptor slot.
            unsafe {
                *output = descriptor;
            }
            ffi::SQLITE_OK
        }
        Err(_) => ffi::SQLITE_CANTOPEN,
    }
}
unsafe extern "C" fn full_path(
    _vfs: *mut ffi::sqlite3_vfs,
    input: *const libc::c_char,
    size: i32,
    output: *mut libc::c_char,
) -> i32 {
    let expected = c"/skill-studio-event/events.sqlite3";
    if input.is_null()
        || output.is_null()
        || unsafe { CStr::from_ptr(input) } != expected
        || size < expected.to_bytes_with_nul().len() as i32
    {
        deny();
        return ffi::SQLITE_CANTOPEN;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            expected.as_ptr(),
            output,
            expected.to_bytes_with_nul().len(),
        );
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn getcwd(_output: *mut libc::c_char, _size: usize) -> *mut libc::c_char {
    deny();
    std::ptr::null_mut()
}
unsafe extern "C" fn readlink(
    _path: *const libc::c_char,
    _output: *mut libc::c_char,
    _size: usize,
) -> isize {
    deny() as isize
}
unsafe extern "C" fn entropy(
    _vfs: *mut ffi::sqlite3_vfs,
    length: i32,
    output: *mut libc::c_char,
) -> i32 {
    if length <= 0 || output.is_null() {
        return 0;
    }
    // SQLite owns the output buffer; getentropy accepts at most 256 bytes per call.
    let bytes = unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), length as usize) };
    for chunk in bytes.chunks_mut(256) {
        if unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) } != 0 {
            return 0;
        }
    }
    length
}

unsafe extern "C" fn sync_file(file: *mut ffi::sqlite3_file, flags: i32) -> i32 {
    let original = {
        let mut state = STATE.get().unwrap().lock().unwrap();
        let Some(methods) = state.io_methods.get(&(file as usize)) else {
            return ffi::SQLITE_IOERR_FSYNC;
        };
        let original = methods.original;
        state.sync_calls += 1;
        #[cfg(test)]
        if state.fail_sync {
            state.sync_failures += 1;
            return ffi::SQLITE_IOERR_FSYNC;
        }
        original
    };
    let method = unsafe { (*(original as *const ffi::sqlite3_io_methods)).xSync };
    match method {
        Some(sync) => unsafe { sync(file, flags) },
        None => ffi::SQLITE_IOERR_FSYNC,
    }
}
unsafe extern "C" fn close_sqlite_file(file: *mut ffi::sqlite3_file) -> i32 {
    let methods = STATE
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .io_methods
        .remove(&(file as usize));
    let Some(methods) = methods else {
        return ffi::SQLITE_IOERR_CLOSE;
    };
    let original = methods.original as *const ffi::sqlite3_io_methods;
    unsafe {
        (*file).pMethods = original;
    }
    let close = unsafe { (*original).xClose };
    let result = match close {
        Some(close) => unsafe { close(file) },
        None => ffi::SQLITE_IOERR_CLOSE,
    };
    drop(methods.methods);
    result
}
unsafe extern "C" fn control_sqlite_file(
    file: *mut ffi::sqlite3_file,
    operation: i32,
    argument: *mut libc::c_void,
) -> i32 {
    if operation == ffi::SQLITE_FCNTL_SET_LOCKPROXYFILE {
        return ffi::SQLITE_NOTFOUND;
    }
    let original = {
        let state = STATE.get().unwrap().lock().unwrap();
        let Some(methods) = state.io_methods.get(&(file as usize)) else {
            return ffi::SQLITE_NOTFOUND;
        };
        methods.original
    };
    let control = unsafe { (*(original as *const ffi::sqlite3_io_methods)).xFileControl };
    match control {
        Some(control) => unsafe { control(file, operation, argument) },
        None => ffi::SQLITE_NOTFOUND,
    }
}
unsafe extern "C" fn open_sqlite_file(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const libc::c_char,
    file: *mut ffi::sqlite3_file,
    flags: i32,
    out_flags: *mut i32,
) -> i32 {
    if flags & ffi::SQLITE_OPEN_AUTOPROXY != 0 {
        return ffi::SQLITE_CANTOPEN;
    }
    if STATE.get().unwrap().lock().unwrap().io_methods.len() >= 16 {
        return ffi::SQLITE_CANTOPEN;
    }
    let result = unsafe { NATIVE_OPEN.get().unwrap()(vfs, name, file, flags, out_flags) };
    if result != ffi::SQLITE_OK || unsafe { (*file).pMethods.is_null() } {
        return result;
    }
    let original = unsafe { (*file).pMethods };
    let mut methods = Box::new(unsafe { original.read() });
    methods.xSync = Some(sync_file);
    methods.xClose = Some(close_sqlite_file);
    methods.xFileControl = Some(control_sqlite_file);
    unsafe {
        (*file).pMethods = methods.as_ref();
    }
    let previous = STATE.get().unwrap().lock().unwrap().io_methods.insert(
        file as usize,
        IoMethods {
            methods,
            original: original as usize,
        },
    );
    assert!(previous.is_none());
    result
}

unsafe fn install() {
    // This child has no prior SQLite connection and exits with hooks installed.
    unsafe {
        assert_eq!(ffi::sqlite3_initialize(), ffi::SQLITE_OK);
        assert_eq!(
            CStr::from_ptr(ffi::sqlite3_sourceid()),
            c"2026-06-03 19:12:13 d6e03d8c777cfa2d35e3b60d8ec3e0187f3e9f99d8e2ee9cac695fd6fcdf1a24"
        );
        assert_eq!(
            ffi::sqlite3_compileoption_used(c"PREFER_PROXY_LOCKING".as_ptr()),
            0
        );
        let name = if !ffi::sqlite3_vfs_find(c"unix-posix".as_ptr()).is_null() {
            c"unix-posix"
        } else {
            assert_eq!(
                ffi::sqlite3_compileoption_used(c"ENABLE_LOCKING_STYLE=0".as_ptr()),
                1
            );
            c"unix"
        };
        assert!(SELECTED_VFS.set(name).is_ok());
        let vfs = ffi::sqlite3_vfs_find(name.as_ptr());
        assert!(!vfs.is_null());
        assert!(NATIVE_OPEN.set((*vfs).xOpen.unwrap()).is_ok());
        (*vfs).xOpen = Some(open_sqlite_file);
        let next = (*vfs).xNextSystemCall.unwrap();
        let get = (*vfs).xGetSystemCall.unwrap();
        let known = [
            "open",
            "close",
            "access",
            "getcwd",
            "stat",
            "fstat",
            "ftruncate",
            "fcntl",
            "read",
            "pread",
            "pread64",
            "write",
            "pwrite",
            "pwrite64",
            "fchmod",
            "fallocate",
            "unlink",
            "openDirectory",
            "mkdir",
            "rmdir",
            "fchown",
            "geteuid",
            "mmap",
            "munmap",
            "mremap",
            "getpagesize",
            "readlink",
            "lstat",
            "ioctl",
        ];
        let mut active = Vec::new();
        let mut name = next(vfs, std::ptr::null());
        while !name.is_null() {
            let text = CStr::from_ptr(name).to_str().unwrap();
            assert!(
                known.contains(&text),
                "unclassified SQLite callback: {text}"
            );
            assert!(!active.contains(&text), "repeated SQLite callback: {text}");
            assert!(get(vfs, name).is_some());
            active.push(text);
            name = next(vfs, name);
        }
        for required in [
            "open", "close", "fstat", "fcntl", "pread", "pwrite", "mmap", "munmap",
        ] {
            assert!(
                active.contains(&required),
                "missing SQLite callback: {required}"
            );
        }
        // These require separate platform policies before this probe can use them.
        for unsupported in ["pread64", "pwrite64", "fallocate", "mremap", "ioctl"] {
            assert!(
                !active.contains(&unsupported),
                "unsupported SQLite callback: {unsupported}"
            );
        }
        let set = (*vfs).xSetSystemCall.unwrap();
        macro_rules! replace {
            ($name:expr, $callback:ident, $signature:ty) => {
                assert_eq!(
                    set(
                        vfs,
                        $name.as_ptr(),
                        Some(std::mem::transmute::<$signature, unsafe extern "C" fn()>(
                            $callback
                        ))
                    ),
                    ffi::SQLITE_OK
                );
            };
        }
        replace!(
            c"open",
            open,
            unsafe extern "C" fn(*const libc::c_char, i32, i32) -> i32
        );
        replace!(c"close", close, unsafe extern "C" fn(i32) -> i32);
        skill_studio_event_set_fd_authorizer(authorize_fcntl);
        replace!(
            c"fcntl",
            skill_studio_event_fcntl,
            unsafe extern "C" fn(i32, i32, ...) -> i32
        );
        replace!(
            c"fstat",
            fstat,
            unsafe extern "C" fn(i32, *mut libc::stat) -> i32
        );
        if active.contains(&"fchmod") {
            replace!(
                c"fchmod",
                fchmod,
                unsafe extern "C" fn(i32, libc::mode_t) -> i32
            );
        }
        if active.contains(&"fchown") {
            replace!(
                c"fchown",
                fchown,
                unsafe extern "C" fn(i32, libc::uid_t, libc::gid_t) -> i32
            );
        }
        replace!(
            c"mkdir",
            mkdir,
            unsafe extern "C" fn(*const libc::c_char, libc::mode_t) -> i32
        );
        replace!(
            c"rmdir",
            rmdir,
            unsafe extern "C" fn(*const libc::c_char) -> i32
        );
        replace!(
            c"mmap",
            mmap,
            unsafe extern "C" fn(
                *mut libc::c_void,
                usize,
                i32,
                i32,
                i32,
                libc::off_t,
            ) -> *mut libc::c_void
        );
        replace!(
            c"munmap",
            munmap,
            unsafe extern "C" fn(*mut libc::c_void, usize) -> i32
        );
        replace!(
            c"read",
            read,
            unsafe extern "C" fn(i32, *mut libc::c_void, usize) -> isize
        );
        replace!(
            c"pread",
            pread,
            unsafe extern "C" fn(i32, *mut libc::c_void, usize, libc::off_t) -> isize
        );
        replace!(
            c"write",
            write,
            unsafe extern "C" fn(i32, *const libc::c_void, usize) -> isize
        );
        replace!(
            c"pwrite",
            pwrite,
            unsafe extern "C" fn(i32, *const libc::c_void, usize, libc::off_t) -> isize
        );
        replace!(
            c"ftruncate",
            truncate,
            unsafe extern "C" fn(i32, libc::off_t) -> i32
        );
        replace!(
            c"stat",
            stat,
            unsafe extern "C" fn(*const libc::c_char, *mut libc::stat) -> i32
        );
        replace!(
            c"lstat",
            stat,
            unsafe extern "C" fn(*const libc::c_char, *mut libc::stat) -> i32
        );
        replace!(
            c"access",
            access,
            unsafe extern "C" fn(*const libc::c_char, i32) -> i32
        );
        replace!(
            c"unlink",
            unlink,
            unsafe extern "C" fn(*const libc::c_char) -> i32
        );
        replace!(
            c"openDirectory",
            open_directory,
            unsafe extern "C" fn(*const libc::c_char, *mut i32) -> i32
        );
        replace!(
            c"getcwd",
            getcwd,
            unsafe extern "C" fn(*mut libc::c_char, usize) -> *mut libc::c_char
        );
        replace!(
            c"readlink",
            readlink,
            unsafe extern "C" fn(*const libc::c_char, *mut libc::c_char, usize) -> isize
        );
        (*vfs).xFullPathname = Some(full_path);
        (*vfs).xRandomness = Some(entropy);
        assert_eq!(ffi::sqlite3_vfs_register(vfs, 1), ffi::SQLITE_OK);
        assert_eq!(ffi::sqlite3_vfs_find(std::ptr::null()), vfs);
    }
}

fn initialize_state(directory: Dir) {
    assert!(STATE
        .set(Mutex::new(State {
            io_methods: HashMap::new(),
            sync_calls: 0,
            #[cfg(test)]
            sync_failures: 0,
            #[cfg(test)]
            fail_sync: false,
            directory,
            descriptors: HashMap::new(),
            denied: false,
            opened_roles: [0; 4],
            descriptor_calls: [0; 3],
            read_calls: [0; 2],
            mappings: HashMap::new(),
            mapped_bytes: 0,
            peak_mapped_bytes: 0,
            mapping_calls: [0; 2],
        }))
        .is_ok());
}

fn process_request(socket: &mut std::os::unix::net::UnixStream) -> serde_json::Value {
    let request = match crate::skill_event_worker_protocol::read_event_request(socket) {
        Ok(request) => request,
        Err(reply) => return serde_json::to_value(reply).expect("event rejection is serializable"),
    };
    let unknown = request.outcome_unknown();
    let result = (|| -> Result<serde_json::Value, ()> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        if request.requires_initialization() {
            let state = STATE.get().unwrap().lock().unwrap();
            match state.directory.symlink_metadata(EventFile::Database.name()) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    for role in [EventFile::Wal, EventFile::SharedMemory, EventFile::Journal] {
                        match state.directory.symlink_metadata(role.name()) {
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            _ => return Err(()),
                        }
                    }
                }
                Err(_) => return Err(()),
            }
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let connection = Connection::open_with_flags_and_vfs(
            "/skill-studio-event/events.sqlite3",
            flags,
            SELECTED_VFS.get().unwrap().to_str().unwrap(),
        )
        .map_err(|_| ())?;
        let mut persist_wal: libc::c_int = 1;
        // Keep the WAL and shared-memory files available to strictly read-only workers after close.
        let configured = unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_PERSIST_WAL,
                (&mut persist_wal as *mut libc::c_int).cast(),
            )
        };
        if configured != ffi::SQLITE_OK {
            return Err(());
        }
        let reply = request.execute_and_close(connection).map_err(|_| ())?;
        let state = STATE.get().unwrap().lock().unwrap();
        if state.denied
            || !state.descriptors.is_empty()
            || !state.mappings.is_empty()
            || !state.io_methods.is_empty()
        {
            return Err(());
        }
        Ok(reply)
    })();
    result.unwrap_or(unknown)
}

/// Serves one request using an already-transferred directory and private socket.
/// The caller must configure IO deadlines and terminate this process afterward.
///
/// # Safety
/// Call only in a dedicated child with no previous SQLite connections and no
/// concurrent SQLite users. This replaces process-global SQLite callbacks and
/// never restores them. The process must exit on return or panic.
pub unsafe fn serve_event_database(
    mut socket: std::os::unix::net::UnixStream,
    directory: Dir,
) -> Result<(), String> {
    initialize_state(directory);
    unsafe {
        install();
    }
    let reply = process_request(&mut socket);
    crate::skill_history_worker_frame::write_json_frame(&mut socket, 64 * 1024, &reply)
        .map_err(|error| format!("Could not write event reply: {error:?}"))
}

#[cfg(test)]
#[path = "skill_event_native_probe.rs"]
mod tests;
