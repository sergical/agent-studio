use super::*;
use crate::skill_scope::SkillReadScope;
#[test]
#[ignore = "private native SQLite subprocess"]
fn native_writer_child() {
    use std::{
        io::{Read, Write},
        os::{fd::AsFd, unix::net::UnixStream},
    };
    let mut socket = UnixStream::from(std::io::stdin().as_fd().try_clone_to_owned().unwrap());
    let directory =
        crate::skill_history_worker_bootstrap::receive_history_directory_or_exit(&socket);
    let mode = std::env::var("SKILL_STUDIO_EVENT_DESCRIPTOR_PROBE").unwrap_or_default();
    if matches!(
        mode.as_str(),
        "framed-production" | "framed-production-no-reply"
    ) {
        socket.write_all(b"R").unwrap();
        let mut proceed = [0];
        socket.read_exact(&mut proceed).unwrap();
        assert_eq!(proceed, [1]);
        if mode == "framed-production-no-reply" {
            initialize_state(directory);
            unsafe {
                install();
            }
            let reply = process_request(&mut socket);
            assert_eq!(reply["outcome"], "committed");
        } else {
            unsafe { serve_event_database(socket, directory) }.unwrap();
        }
        return;
    }
    initialize_state(directory);
    socket.write_all(b"R").unwrap();
    let mut proceed = [0];
    socket.read_exact(&mut proceed).unwrap();
    assert_eq!(proceed, [1]);
    unsafe {
        install();
    }
    let descriptor_probe = std::env::var("SKILL_STUDIO_EVENT_DESCRIPTOR_PROBE").ok();
    STATE.get().unwrap().lock().unwrap().fail_sync =
        descriptor_probe.as_deref() == Some("framed-sync-failure");
    if descriptor_probe.as_deref() == Some("methods") {
        let connection = Connection::open_with_flags_and_vfs(
            "/skill-studio-event/events.sqlite3",
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            SELECTED_VFS.get().unwrap().to_str().unwrap(),
        )
        .unwrap();
        connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(value INTEGER); INSERT INTO probe VALUES(1);").unwrap();
        let before = STATE.get().unwrap().lock().unwrap().opened_roles;
        let result = unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_SET_LOCKPROXYFILE,
                c"unsupported-proxy".as_ptr().cast_mut().cast(),
            )
        };
        assert_eq!(result, ffi::SQLITE_NOTFOUND);
        assert_eq!(STATE.get().unwrap().lock().unwrap().opened_roles, before);
        let mut lock_state: i32 = -1;
        assert_eq!(
            unsafe {
                ffi::sqlite3_file_control(
                    connection.handle(),
                    c"main".as_ptr(),
                    ffi::SQLITE_FCNTL_LOCKSTATE,
                    (&mut lock_state as *mut i32).cast(),
                )
            },
            ffi::SQLITE_OK
        );
        assert!(lock_state >= 0);
        connection.close().unwrap();
        let state = STATE.get().unwrap().lock().unwrap();
        assert!(
            state.io_methods.is_empty()
                && state.descriptors.is_empty()
                && state.mappings.is_empty()
        );
        socket.write_all(format!("{}\n", serde_json::json!({"proxy_switch_refused":true,"vfs":SELECTED_VFS.get().unwrap().to_str().unwrap()})).as_bytes()).unwrap();
        return;
    }
    if descriptor_probe.as_deref() == Some("fcntl") {
        let database = unsafe {
            open(
                c"/skill-studio-event/events.sqlite3".as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        let wal = unsafe {
            open(
                c"/skill-studio-event/events.sqlite3-wal".as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        assert!(database >= 0 && wal >= 0);
        assert_eq!(
            unsafe { skill_studio_event_fcntl(database, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
        assert_eq!(
            unsafe { skill_studio_event_fcntl(database, libc::F_GETFD) },
            libc::FD_CLOEXEC
        );
        assert_eq!(
            unsafe { skill_studio_event_fcntl(database, libc::F_SETFD, 0) },
            -1
        );
        assert_eq!(
            unsafe { skill_studio_event_fcntl(database, libc::F_DUPFD) },
            -1
        );
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK;
        lock.l_whence = libc::SEEK_SET as i16;
        lock.l_len = 1;
        lock.l_start = 0x4000_0000;
        assert_eq!(
            unsafe { skill_studio_event_fcntl(wal, libc::F_SETLK, &mut lock as *mut libc::flock) },
            -1
        );
        assert_eq!(
            unsafe {
                skill_studio_event_fcntl(database, libc::F_SETLK, &mut lock as *mut libc::flock)
            },
            0
        );
        lock.l_type = libc::F_UNLCK;
        assert_eq!(
            unsafe {
                skill_studio_event_fcntl(database, libc::F_SETLK, &mut lock as *mut libc::flock)
            },
            0
        );
        for (start, len, whence, kind) in [
            (0, 1, libc::SEEK_SET as i16, libc::F_WRLCK),
            (0x4000_0000, 0, libc::SEEK_SET as i16, libc::F_WRLCK),
            (0x4000_0000, -1, libc::SEEK_SET as i16, libc::F_WRLCK),
            (i64::MAX, 2, libc::SEEK_SET as i16, libc::F_WRLCK),
            (0x4000_0200, 1, libc::SEEK_SET as i16, libc::F_WRLCK),
            (0x4000_0000, 1, libc::SEEK_CUR as i16, libc::F_WRLCK),
            (0x4000_0000, 1, libc::SEEK_SET as i16, 99),
        ] {
            lock.l_start = start;
            lock.l_len = len;
            lock.l_whence = whence;
            lock.l_type = kind;
            assert_eq!(
                unsafe {
                    skill_studio_event_fcntl(database, libc::F_SETLK, &mut lock as *mut libc::flock)
                },
                -1
            );
        }
        let shm = unsafe {
            open(
                c"/skill-studio-event/events.sqlite3-shm".as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        assert!(shm >= 0);
        for (start, len, expected) in [
            (120, 8, 0),
            (128, 1, 0),
            (127, 2, -1),
            (119, 1, -1),
            (129, 1, -1),
        ] {
            lock.l_start = start;
            lock.l_len = len;
            lock.l_whence = libc::SEEK_SET as i16;
            lock.l_type = libc::F_WRLCK;
            assert_eq!(
                unsafe {
                    skill_studio_event_fcntl(shm, libc::F_SETLK, &mut lock as *mut libc::flock)
                },
                expected
            );
            if expected == 0 {
                lock.l_type = libc::F_UNLCK;
                assert_eq!(
                    unsafe {
                        skill_studio_event_fcntl(shm, libc::F_SETLK, &mut lock as *mut libc::flock)
                    },
                    0
                );
            }
        }
        assert_eq!(unsafe { close(shm) }, 0);
        let outside = tempfile::tempfile().unwrap();
        assert_eq!(
            unsafe { skill_studio_event_fcntl(outside.as_raw_fd(), libc::F_GETFD) },
            -1
        );
        let mut directory = -1;
        assert_eq!(
            unsafe {
                open_directory(
                    c"/skill-studio-event/events.sqlite3".as_ptr(),
                    &mut directory,
                )
            },
            ffi::SQLITE_OK
        );
        assert!(valid_directory_descriptor(directory));
        assert!(unsafe { skill_studio_event_fcntl(directory, libc::F_GETFD) } >= 0);
        assert_eq!(
            unsafe {
                skill_studio_event_fcntl(directory, libc::F_GETLK, &mut lock as *mut libc::flock)
            },
            -1
        );
        assert_eq!(unsafe { close(directory) }, 0);
        assert_eq!(unsafe { close(wal) }, 0);
        assert_eq!(unsafe { close(database) }, 0);
        socket.write_all(b"{\"fcntl_policy\":true}\n").unwrap();
        return;
    }
    if descriptor_probe.as_deref() == Some("permissions") {
        let database = unsafe {
            open(
                c"/skill-studio-event/events.sqlite3".as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        assert!(database >= 0);
        assert_eq!(unsafe { libc::fchmod(database, 0o600) }, 0);
        let sidecar = unsafe {
            open(
                c"/skill-studio-event/events.sqlite3-wal".as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        assert!(sidecar >= 0);
        assert_eq!(unsafe { libc::fchmod(sidecar, 0o644) }, 0);
        let metadata = file_metadata(database).unwrap();
        assert_eq!(unsafe { fchmod(sidecar, 0o600) }, 0);
        assert_eq!(file_metadata(sidecar).unwrap().st_mode & 0o777, 0o600);
        assert_eq!(
            unsafe { fchown(sidecar, metadata.st_uid, metadata.st_gid) },
            0
        );
        assert_eq!(unsafe { fchmod(sidecar, 0o644) }, -1);
        assert_eq!(unsafe { fchmod(sidecar, 0o4600) }, -1);
        assert_eq!(unsafe { fchmod(database, 0o600) }, -1);
        assert_eq!(
            unsafe { fchown(database, metadata.st_uid, metadata.st_gid) },
            -1
        );
        assert_eq!(
            unsafe { fchown(sidecar, metadata.st_uid.wrapping_add(1), metadata.st_gid) },
            -1
        );
        assert_eq!(
            unsafe { fchown(sidecar, metadata.st_uid, metadata.st_gid.wrapping_add(1)) },
            -1
        );
        let outside = tempfile::tempfile().unwrap();
        use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
        let before = outside.metadata().unwrap();
        assert_eq!(unsafe { fchmod(outside.as_raw_fd(), 0o600) }, -1);
        assert_eq!(
            unsafe { fchown(outside.as_raw_fd(), metadata.st_uid, metadata.st_gid) },
            -1
        );
        let after = outside.metadata().unwrap();
        assert_eq!(
            (after.mode(), after.uid(), after.gid()),
            (before.mode(), before.uid(), before.gid())
        );
        assert_eq!(unsafe { truncate(sidecar, 1) }, 0);
        assert_eq!(unsafe { fchmod(sidecar, 0o600) }, -1);
        assert_eq!(file_metadata(sidecar).unwrap().st_mode & 0o777, 0o600);
        assert_eq!(unsafe { close(sidecar) }, 0);
        assert_eq!(unsafe { close(database) }, 0);
        socket.write_all(b"{\"permission_policy\":true}\n").unwrap();
        return;
    }
    if descriptor_probe.as_deref() == Some("directories") {
        let state = STATE.get().unwrap().lock().unwrap();
        state.directory.create_dir("existing").unwrap();
        drop(state);
        assert_eq!(
            unsafe { mkdir(c"/skill-studio-event/new".as_ptr(), 0o700) },
            -1
        );
        assert_eq!(
            unsafe { rmdir(c"/skill-studio-event/existing".as_ptr()) },
            -1
        );
        let state = STATE.get().unwrap().lock().unwrap();
        assert!(state
            .directory
            .symlink_metadata("existing")
            .unwrap()
            .is_dir());
        assert!(state.directory.symlink_metadata("new").is_err());
        socket
            .write_all(b"{\"directories_refused\":true}\n")
            .unwrap();
        return;
    }

    if matches!(
        descriptor_probe.as_deref(),
        Some("mapping" | "mapping-limits")
    ) {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        assert!(page > 0);
        let fd = unsafe {
            open(
                c"/skill-studio-event/events.sqlite3-shm".as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        assert!(fd >= 0);
        let length = if descriptor_probe.as_deref() == Some("mapping-limits") {
            MAX_MAPPED_BYTES
        } else {
            page
        };
        assert_eq!(unsafe { truncate(fd, length as i64) }, 0);
        if descriptor_probe.as_deref() == Some("mapping") {
            let mapped = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    page,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            assert_ne!(mapped, libc::MAP_FAILED);
            // The approved SHM region is writable, but a partial unmap is not authorized.
            unsafe {
                *mapped.cast::<u8>() = 42;
            }
            assert_eq!(unsafe { munmap(mapped, page - 1) }, -1);
            assert_eq!(unsafe { *mapped.cast::<u8>() }, 42);
            assert_eq!(
                unsafe {
                    mmap(
                        std::ptr::null_mut(),
                        page,
                        libc::PROT_READ | libc::PROT_EXEC,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    )
                },
                libc::MAP_FAILED
            );
            assert_eq!(
                unsafe {
                    mmap(
                        mapped,
                        page,
                        libc::PROT_READ,
                        libc::MAP_SHARED | libc::MAP_FIXED,
                        fd,
                        0,
                    )
                },
                libc::MAP_FAILED
            );
            assert_eq!(unsafe { *mapped.cast::<u8>() }, 42);
            assert_eq!(unsafe { munmap(mapped, page) }, 0);
            let foreign = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    page,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANON,
                    -1,
                    0,
                )
            };
            assert_ne!(foreign, libc::MAP_FAILED);
            unsafe {
                *foreign.cast::<u8>() = 43;
            }
            assert_eq!(unsafe { munmap(foreign, page) }, -1);
            assert_eq!(unsafe { *foreign.cast::<u8>() }, 43);
            assert_eq!(unsafe { libc::munmap(foreign, page) }, 0);
        } else {
            let mapped = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            assert_ne!(mapped, libc::MAP_FAILED);
            assert_eq!(
                unsafe {
                    mmap(
                        std::ptr::null_mut(),
                        page,
                        libc::PROT_READ,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    )
                },
                libc::MAP_FAILED
            );
            assert_eq!(unsafe { munmap(mapped, length) }, 0);
            let maps: Vec<_> = (0..MAX_MAPPINGS)
                .map(|_| unsafe {
                    mmap(
                        std::ptr::null_mut(),
                        page,
                        libc::PROT_READ,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    )
                })
                .collect();
            assert!(maps.iter().all(|mapped| *mapped != libc::MAP_FAILED));
            assert_eq!(
                unsafe {
                    mmap(
                        std::ptr::null_mut(),
                        page,
                        libc::PROT_READ,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    )
                },
                libc::MAP_FAILED
            );
            for mapped in maps {
                assert_eq!(unsafe { munmap(mapped, page) }, 0);
            }
        }
        assert_eq!(unsafe { close(fd) }, 0);
        let state = STATE.get().unwrap().lock().unwrap();
        assert!(
            state.mappings.is_empty() && state.mapped_bytes == 0 && state.descriptors.is_empty()
        );
        socket
            .write_all(b"{\"mapping_policy_passed\":true}\n")
            .unwrap();
        return;
    }
    if matches!(descriptor_probe.as_deref(), Some("replaced" | "linked")) {
        let path = c"/skill-studio-event/events.sqlite3-wal";
        let original = b"original";
        // The test actor changes a file after the scoped opener has admitted it.
        let fd = unsafe {
            open(
                path.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        assert!(fd >= 0);
        assert_eq!(
            unsafe { pwrite(fd, original.as_ptr().cast(), original.len(), 0) },
            original.len() as isize
        );
        {
            let state = STATE.get().unwrap().lock().unwrap();
            if descriptor_probe.as_deref() == Some("replaced") {
                state
                    .directory
                    .rename(EventFile::Wal.name(), &state.directory, "previous")
                    .unwrap();
                state
                    .directory
                    .write(EventFile::Wal.name(), b"replacement")
                    .unwrap();
            } else {
                state
                    .directory
                    .hard_link(EventFile::Wal.name(), &state.directory, "linked-copy")
                    .unwrap();
            }
        }
        let replacement = b"X";
        assert_reads_refused(fd);
        unsafe {
            assert_eq!(write(fd, replacement.as_ptr().cast(), 1), -1);
            assert_eq!(pwrite(fd, replacement.as_ptr().cast(), 1, 0), -1);
            assert_eq!(truncate(fd, 0), -1);
            assert_eq!(close(fd), 0);
        }
        let state = STATE.get().unwrap().lock().unwrap();
        if descriptor_probe.as_deref() == Some("replaced") {
            assert_eq!(state.directory.read("previous").unwrap(), original);
            assert_eq!(
                state.directory.read(EventFile::Wal.name()).unwrap(),
                b"replacement"
            );
        } else {
            assert_eq!(state.directory.read("linked-copy").unwrap(), original);
            assert_eq!(
                state.directory.read(EventFile::Wal.name()).unwrap(),
                original
            );
        }
        assert!(state.denied && state.descriptors.is_empty());
        socket
            .write_all(b"{\"stale_descriptor_refused\":true}\n")
            .unwrap();
        return;
    }
    if descriptor_probe.as_deref() == Some("reused") {
        let fd = unsafe {
            open(
                c"/skill-studio-event/events.sqlite3-wal".as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        assert!(fd >= 0);
        // Simulate a stale registry entry without asking the scoped closer to retire it.
        assert_eq!(unsafe { libc::close(fd) }, 0);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("foreign");
        std::fs::write(&path, b"foreign sentinel").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        if file.as_raw_fd() != fd {
            assert_eq!(unsafe { libc::dup2(file.as_raw_fd(), fd) }, fd);
        }
        let byte = b"X";
        assert_reads_refused(fd);
        unsafe {
            assert_eq!(pwrite(fd, byte.as_ptr().cast(), 1, 0), -1);
            assert_eq!(close(fd), -1);
        }
        assert!(file.metadata().is_ok());
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        assert_eq!(unsafe { libc::fstat(fd, metadata.as_mut_ptr()) }, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"foreign sentinel");
        if file.as_raw_fd() != fd {
            assert_eq!(unsafe { libc::close(fd) }, 0);
        }
        socket
            .write_all(b"{\"reused_descriptor_refused\":true}\n")
            .unwrap();
        return;
    }
    if descriptor_probe.as_deref() == Some("foreign") {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("outside");
        std::fs::write(&path, b"outside sentinel").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let fd = file.as_raw_fd();
        let byte = b"X";
        assert_reads_refused(fd);
        // Deliberately pass a descriptor that never came from the scoped opener.
        unsafe {
            assert_eq!(write(fd, byte.as_ptr().cast(), 1), -1);
            assert_eq!(pwrite(fd, byte.as_ptr().cast(), 1, 0), -1);
            assert_eq!(truncate(fd, 0), -1);
            assert_eq!(close(fd), -1);
        }
        assert!(file.metadata().is_ok());
        assert_eq!(std::fs::read(&path).unwrap(), b"outside sentinel");
        socket
            .write_all(b"{\"foreign_descriptor_refused\":true}\n")
            .unwrap();
        return;
    }
    if matches!(
        descriptor_probe.as_deref(),
        Some(
            "framed-finish"
                | "framed-production"
                | "framed-lost-reply"
                | "framed-partial-reply"
                | "framed-wrong-reply"
                | "framed-hold-exit"
                | "framed-sync-failure"
        )
    ) {
        use crate::skill_history_worker_frame::write_json_frame;
        let mut reply = process_request(&mut socket);
        if reply["outcome"] == "rejected_before_open" {
            let state = STATE.get().unwrap().lock().unwrap();
            assert!(state.opened_roles.iter().all(|count| *count == 0));
            assert!(state.descriptors.is_empty() && state.mappings.is_empty());
        }
        if descriptor_probe.as_deref() == Some("framed-sync-failure") {
            let state = STATE.get().unwrap().lock().unwrap();
            assert!(state.sync_failures > 0);
            assert!(state.io_methods.is_empty());
            assert_eq!(reply["outcome"], "unknown");
        }
        match descriptor_probe.as_deref() {
            Some("framed-lost-reply") => {
                assert_eq!(reply["outcome"], "committed");
                unsafe { libc::_exit(0) };
            }
            Some("framed-partial-reply") => {
                assert_eq!(reply["outcome"], "committed");
                socket.write_all(&[0, 0, 0, 20, b'{']).unwrap();
                return;
            }
            Some("framed-wrong-reply") => {
                assert_eq!(reply["outcome"], "committed");
                reply["exchange_id"] = serde_json::json!("wrong-exchange");
            }
            _ => {}
        }
        write_json_frame(&mut socket, 64 * 1024, &reply).unwrap();
        if descriptor_probe.as_deref() == Some("framed-hold-exit") {
            assert_eq!(reply["outcome"], "committed");
            socket.shutdown(std::net::Shutdown::Write).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        return;
    }
    let result = (|| -> Result<i64, String> {
        let mut connection = Connection::open_with_flags_and_vfs(
            "/skill-studio-event/events.sqlite3",
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            SELECTED_VFS.get().unwrap().to_str().unwrap(),
        )
        .map_err(|error| error.to_string())?;
        if matches!(
            descriptor_probe.as_deref(),
            Some("receipt-commit-exit" | "receipt-replay")
        ) {
            connection.execute_batch("PRAGMA synchronous=FULL").unwrap();
            let count =
                crate::skill_event_worker_protocol::exercise_native_receipts(&mut connection)?;
            assert_eq!(count, 2);
            if descriptor_probe.as_deref() == Some("receipt-commit-exit") {
                let state = STATE.get().unwrap().lock().unwrap();
                assert!(!state.denied);
                assert!(!state.descriptors.is_empty());
                socket
                    .write_all(b"{\"receipt_commit_checkpoint\":true}\n")
                    .unwrap();
                unsafe { libc::_exit(0) };
            }
            connection.close().map_err(|(_, error)| error.to_string())?;
            return Ok(count);
        }
        if matches!(
            descriptor_probe.as_deref(),
            Some("exit-before-commit" | "exit-after-commit")
        ) {
            connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA cache_size=2; BEGIN IMMEDIATE; WITH RECURSIVE numbers(value) AS (SELECT 1 UNION ALL SELECT value+1 FROM numbers WHERE value<5000) INSERT INTO probe SELECT value FROM numbers;").unwrap();
            let committed = descriptor_probe.as_deref() == Some("exit-after-commit");
            if committed {
                connection.execute_batch("COMMIT").unwrap();
            }
            let state = STATE.get().unwrap().lock().unwrap();
            assert!(!state.denied);
            assert!(state.descriptor_calls[1] > 0);
            assert!(!state.descriptors.is_empty());
            let report = serde_json::json!({"exit_stage": descriptor_probe, "positional_writes": state.descriptor_calls[1], "live_descriptors": state.descriptors.len()});
            socket.write_all(format!("{report}\n").as_bytes()).unwrap();
            // Bypass Connection::drop, rollback, checkpoint and Rust cleanup.
            unsafe { libc::_exit(0) };
        }
        if matches!(
            descriptor_probe.as_deref(),
            Some("lock-busy" | "lock-commit")
        ) {
            connection.busy_timeout(std::time::Duration::ZERO).unwrap();
            let begin = connection.execute_batch("PRAGMA synchronous=FULL; BEGIN IMMEDIATE;");
            if descriptor_probe.as_deref() == Some("lock-busy") {
                let error = begin.expect_err("another process holds the write lock");
                assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
                assert!(connection.is_autocommit());
                connection.close().map_err(|(_, error)| error.to_string())?;
                return Ok(0);
            }
            begin.map_err(|error| error.to_string())?;
            connection
                .execute("INSERT INTO probe VALUES (42)", [])
                .map_err(|error| error.to_string())?;
            connection
                .execute_batch("COMMIT")
                .map_err(|error| error.to_string())?;
            connection.close().map_err(|(_, error)| error.to_string())?;
            return Ok(42);
        }
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE probe(value INTEGER); BEGIN IMMEDIATE; INSERT INTO probe VALUES (42); COMMIT;").map_err(|error| error.to_string())?;
        let journal: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let synchronous: i64 = connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if journal != "wal" || synchronous != 2 {
            return Err("WAL/FULL policy was not applied".into());
        }
        let value = connection
            .query_row("SELECT value FROM probe", [], |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        connection.close().map_err(|(_, error)| error.to_string())?;
        Ok(value)
    })();
    let state = STATE.get().unwrap().lock().unwrap();
    let report = serde_json::json!({"result": result, "sync_calls": state.sync_calls, "open_method_wrappers": state.io_methods.len(), "denied": state.denied, "open_descriptors": state.descriptors.len(), "opened_roles": state.opened_roles, "descriptor_calls": state.descriptor_calls, "read_calls": state.read_calls, "mapping_calls": state.mapping_calls, "open_mappings": state.mappings.len(), "mapped_bytes": state.mapped_bytes, "peak_mapped_bytes": state.peak_mapped_bytes});
    socket.write_all(format!("{report}\n").as_bytes()).unwrap();
}

fn run_writer(root: &std::path::Path, descriptor_probe: Option<&str>) -> serde_json::Value {
    run_writer_with_frame(root, descriptor_probe, None)
}

fn run_writer_with_frame(
    root: &std::path::Path,
    descriptor_probe: Option<&str>,
    frame: Option<&[u8]>,
) -> serde_json::Value {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    let scope = SkillReadScope::bind(&[root.to_path_buf()]).unwrap();
    let lease = CoordinationPlan::new_fixture(
        vec![DirectoryEffect::tree(root, CoordinationMode::Exclusive)],
        root,
        None,
    )
    .unwrap()
    .acquire()
    .unwrap()
    .finalize_write(&scope, &[])
    .unwrap();
    let reaped = run_leased_writer(root, lease, descriptor_probe, frame);
    let report = match reaped.outcome {
        Ok(report) => report,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    assert!(reaped.exit.success());
    drop(reaped.lease);
    report
}

fn run_leased_writer<'scope>(
    root: &std::path::Path,
    lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    descriptor_probe: Option<&str>,
    frame: Option<&[u8]>,
) -> crate::skill_event_worker_cleanup::ReapedOperation<'scope, serde_json::Value> {
    run_leased_writer_controlled(
        root,
        lease,
        descriptor_probe,
        frame,
        &crate::skill_coordination::CancellationToken::default(),
    )
}

fn run_leased_writer_controlled<'scope>(
    root: &std::path::Path,
    lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    descriptor_probe: Option<&str>,
    frame: Option<&[u8]>,
    cancellation: &crate::skill_coordination::CancellationToken,
) -> crate::skill_event_worker_cleanup::ReapedOperation<'scope, serde_json::Value> {
    let expected = if frame.is_none() {
        Some(crate::skill_event_worker_protocol::prepare_event_request(
            serde_json::from_value(serde_json::json!({"version":1,"action":"apply","exchange_id":"exchange","command_id":"framed-command","event_id":"framed-event","operation":{"kind":"finish_pending","status":"done","inverse":{"post":42}}})).unwrap()
        ).unwrap())
    } else {
        None
    };
    run_leased_exchange(
        root,
        lease,
        descriptor_probe,
        frame,
        cancellation,
        expected.as_ref(),
    )
}

fn run_leased_exchange<'scope>(
    root: &std::path::Path,
    lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    descriptor_probe: Option<&str>,
    frame: Option<&[u8]>,
    cancellation: &crate::skill_coordination::CancellationToken,
    expected: Option<&crate::skill_event_worker_protocol::PreparedEventExchange>,
) -> crate::skill_event_worker_cleanup::ReapedOperation<'scope, serde_json::Value> {
    assert!(frame.is_some() != expected.is_some());
    use crate::skill_history_worker_process::HistoryWorkerProcess;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        process::Command,
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "skill_event_native::tests::native_writer_child",
        "--ignored",
        "--nocapture",
    ]);
    command.env_remove("SKILL_STUDIO_EVENT_DESCRIPTOR_PROBE");
    if let Some(probe) = descriptor_probe {
        command.env(
            "SKILL_STUDIO_EVENT_DESCRIPTOR_PROBE",
            if probe == "framed-cancel-prefix" {
                "framed-finish"
            } else {
                probe
            },
        );
    }
    let mut child = HistoryWorkerProcess::spawn(&mut command).unwrap();
    let mut dispatch = crate::skill_event_worker_dispatch::DispatchState::NotSent;
    let reaped = crate::skill_event_worker_cleanup::run_and_reap(
        &mut child,
        lease,
        |child, lease| {
            if cancellation.is_cancelled() {
                return serde_json::json!({"outcome":"not_started","code":"cancelled"});
            }
            let authority =
                crate::skill_event_file_authority::EventFileAuthority::bind(lease, root).unwrap();
            let transfer = authority.prepare_transfer().unwrap();
            child
                .socket()
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            transfer.send(child.socket()).unwrap();
            let mut ready = [0];
            child.socket().read_exact(&mut ready).unwrap();
            assert_eq!(ready, *b"R");
            authority.revalidate().unwrap();
            child.socket().write_all(&[1]).unwrap();
            if matches!(
                descriptor_probe,
                Some(
                    "framed-finish"
                        | "framed-production"
                        | "framed-cancel-prefix"
                        | "framed-lost-reply"
                        | "framed-partial-reply"
                        | "framed-wrong-reply"
                        | "framed-hold-exit"
                        | "framed-sync-failure"
                )
            ) {
                use crate::skill_event_worker_protocol::EventReply;
                use crate::skill_history_worker_frame::{finish_history_frames, read_json_frame};
                let mut channel = crate::skill_worker_socket::WorkerSocket::new(
                    child.socket(),
                    || cancellation.is_cancelled(),
                    deadline,
                )
                .unwrap();
                if let Some(frame) = frame {
                    child.socket().write_all(frame).unwrap();
                } else {
                    let request = expected.expect("typed request must be prepared").request();
                    struct CancelAfterPrefix<'a> {
                        socket: &'a std::os::unix::net::UnixStream,
                        cancellation: &'a crate::skill_coordination::CancellationToken,
                    }
                    impl Write for CancelAfterPrefix<'_> {
                        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                            let count = self.socket.write(&bytes[..bytes.len().min(2)])?;
                            self.cancellation.cancel();
                            Ok(count)
                        }
                        fn flush(&mut self) -> std::io::Result<()> {
                            Ok(())
                        }
                    }
                    let sent = if descriptor_probe == Some("framed-cancel-prefix") {
                        crate::skill_event_worker_dispatch::send_frame_controlled(
                            &mut CancelAfterPrefix {
                                socket: child.socket(),
                                cancellation,
                            },
                            &mut dispatch,
                            1024 * 1024,
                            &request,
                            cancellation,
                        )
                    } else {
                        crate::skill_event_worker_dispatch::send_frame_controlled(
                            &mut channel,
                            &mut dispatch,
                            1024 * 1024,
                            &request,
                            cancellation,
                        )
                    };
                    if sent.is_err() {
                        return match dispatch {
                            crate::skill_event_worker_dispatch::DispatchState::NotSent => {
                                serde_json::json!({"outcome":"not_started","code":"send_refused"})
                            }
                            crate::skill_event_worker_dispatch::DispatchState::MayHaveSent => {
                                expected.as_ref().unwrap().unavailable_reply()
                            }
                        };
                    }
                    assert_eq!(
                        dispatch,
                        crate::skill_event_worker_dispatch::DispatchState::MayHaveSent
                    );
                }
                child.socket().shutdown(std::net::Shutdown::Write).unwrap();
                let reply: Result<EventReply, _> = read_json_frame(&mut channel, 64 * 1024)
                    .and_then(|reply| finish_history_frames(&mut channel).map(|()| reply));
                if reply.is_err() {
                    return expected
                        .as_ref()
                        .expect("invalid-request fixture must receive rejection")
                        .unavailable_reply();
                }
                if descriptor_probe == Some("framed-hold-exit") {
                    cancellation.cancel();
                }
                if !matches!(crate::skill_event_worker_cleanup::wait_for_exit_controlled(child, cancellation, deadline), Ok(status) if status.success())
                {
                    return expected
                        .as_ref()
                        .expect("invalid-request fixture must receive rejection")
                        .unavailable_reply();
                }
                return match reply.ok().and_then(|reply| reply.validate(expected).ok()) {
                    Some(reply) => reply,
                    None => expected
                        .as_ref()
                        .expect("invalid-request fixture must receive rejection")
                        .unavailable_reply(),
                };
            }
            let mut report = String::new();
            BufReader::new(child.socket())
                .read_line(&mut report)
                .unwrap();
            child
                .wait_for_exit(&AtomicBool::new(false), deadline)
                .unwrap();
            serde_json::from_str(&report).unwrap()
        },
    );
    reaped
}

#[test]
fn native_writer_commits_through_transferred_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let report = run_writer(&root, None);
    println!("{report}");
    assert_eq!(report["result"]["Ok"], 42, "{report}");
    assert!(report["opened_roles"]
        .as_array()
        .unwrap()
        .iter()
        .all(|count| count.as_u64().unwrap() > 0));
    assert_eq!(report["denied"], false);
    assert!(report["sync_calls"].as_u64().unwrap() > 0);
    assert_eq!(report["open_method_wrappers"], 0);
    assert!(report["mapping_calls"][0].as_u64().unwrap() > 0);
    assert_eq!(report["open_mappings"], 0);
    assert_eq!(report["mapped_bytes"], 0);
    assert!(
        report["read_calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|count| count.as_u64().unwrap())
            .sum::<u64>()
            > 0
    );
    assert!(report["descriptor_calls"][1].as_u64().unwrap() > 0);
    assert!(report["descriptor_calls"][2].as_u64().unwrap() > 0);
    assert_eq!(report["open_descriptors"], 0);
    let connection = Connection::open_with_flags(
        root.join("events.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM probe", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        42
    );
}

#[test]
fn native_writer_refuses_linked_database_and_sidecar_entries() {
    use std::fs;
    for role in [
        EventFile::Database,
        EventFile::Wal,
        EventFile::SharedMemory,
        EventFile::Journal,
    ] {
        for hardlink in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let state = root.join("state");
            fs::create_dir(&state).unwrap();
            let outside = root.join("outside");
            fs::write(&outside, b"outside sentinel").unwrap();
            let path = state.join(role.name());
            if hardlink {
                fs::hard_link(&outside, &path).unwrap();
            } else {
                std::os::unix::fs::symlink(&outside, &path).unwrap();
            }
            let report = run_writer(&state, None);
            assert!(
                report["result"]["Err"].is_string(),
                "{}: {report}",
                role.name()
            );
            assert_eq!(report["denied"], true, "{}: {report}", role.name());
            assert_eq!(report["open_descriptors"], 0, "{report}");
            assert_eq!(fs::read(&outside).unwrap(), b"outside sentinel");
        }
    }
}

#[test]
fn native_writer_refuses_foreign_descriptor_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let report = run_writer(&root, Some("foreign"));
    assert_eq!(report["foreign_descriptor_refused"], true);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn native_writer_refuses_changed_registered_files() {
    for probe in ["replaced", "linked"] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let report = run_writer(&root, Some(probe));
        assert_eq!(report["stale_descriptor_refused"], true);
    }
}

#[test]
fn native_writer_refuses_reused_descriptor_number() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let report = run_writer(&root, Some("reused"));
    assert_eq!(report["reused_descriptor_refused"], true);
}

#[test]
fn native_writer_mapping_policy_and_budgets() {
    for probe in ["mapping", "mapping-limits"] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let report = run_writer(&root, Some(probe));
        assert_eq!(report["mapping_policy_passed"], true);
    }
}

#[test]
fn native_writer_refuses_directory_operations() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let report = run_writer(&root, Some("directories"));
    assert_eq!(report["directories_refused"], true);
}

#[test]
fn native_writer_respects_cross_process_write_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let holder = Connection::open(root.join("events.sqlite3")).unwrap();
    holder.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE probe(value INTEGER); BEGIN IMMEDIATE; INSERT INTO probe VALUES (7);").unwrap();
    let busy = run_writer(&root, Some("lock-busy"));
    assert_eq!(busy["result"]["Ok"], 0, "{busy}");
    assert_eq!(busy["denied"], false);
    assert_eq!(busy["open_descriptors"], 0);
    assert_eq!(busy["open_mappings"], 0);
    assert!(!holder.is_autocommit());
    assert_eq!(
        holder
            .query_row("SELECT value FROM probe", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    holder.execute_batch("ROLLBACK").unwrap();
    let committed = run_writer(&root, Some("lock-commit"));
    assert_eq!(committed["result"]["Ok"], 42, "{committed}");
    assert_eq!(committed["denied"], false);
    assert_eq!(committed["open_descriptors"], 0);
    assert_eq!(committed["open_mappings"], 0);
    let values = holder
        .prepare("SELECT value FROM probe")
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(values, vec![42]);
    holder.close().unwrap();
}

#[test]
fn native_writer_recovers_after_exit_without_sqlite_close() {
    for committed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let database = root.join("events.sqlite3");
        let setup = Connection::open(&database).unwrap();
        setup.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(value INTEGER); INSERT INTO probe VALUES (-1);").unwrap();
        setup.close().unwrap();
        let stage = if committed {
            "exit-after-commit"
        } else {
            "exit-before-commit"
        };
        let report = run_writer(&root, Some(stage));
        assert_eq!(report["exit_stage"], stage);
        assert!(report["positional_writes"].as_u64().unwrap() > 0);
        assert!(report["live_descriptors"].as_u64().unwrap() > 0);
        assert!(
            std::fs::metadata(root.join("events.sqlite3-wal"))
                .unwrap()
                .len()
                > 32
        );
        let recovered = Connection::open(&database).unwrap();
        let integrity: String = recovered
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        let (count, total): (i64, i64) = recovered
            .query_row("SELECT count(*), sum(value) FROM probe", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(
            (count, total),
            if committed {
                (5001, 12_502_499)
            } else {
                (1, -1)
            }
        );
        recovered
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO probe VALUES (6000); COMMIT;")
            .unwrap();
        recovered.close().unwrap();
    }
}

#[test]
fn native_writer_replays_receipts_after_unclosed_process_exit() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let setup = crate::skill_event_store::EventStore::open(&root).unwrap();
    drop(setup);
    let exited = run_writer(&root, Some("receipt-commit-exit"));
    assert_eq!(exited["receipt_commit_checkpoint"], true);
    assert!(
        std::fs::metadata(root.join("events.sqlite3-wal"))
            .unwrap()
            .len()
            > 32
    );
    let replay = run_writer(&root, Some("receipt-replay"));
    assert_eq!(replay["result"]["Ok"], 2, "{replay}");
    assert_eq!(replay["denied"], false);
    assert_eq!(replay["open_descriptors"], 0);
    assert_eq!(replay["open_mappings"], 0);
}

#[test]
fn production_native_entry_completes_and_replays_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let setup = crate::skill_event_store::EventStore::open(&root).unwrap();
    setup
        .record(
            "framed-event",
            crate::skill_event::EventDraft {
                kind: "fixture".into(),
                skill: "fixture".into(),
                harness: None,
                scope: None,
                project_path: None,
                payload: serde_json::json!({}),
                inverse: None,
                backup_dir: None,
                restorable: false,
            },
        )
        .unwrap();
    drop(setup);
    let first = run_writer(&root, Some("framed-production"));
    let replay = run_writer(&root, Some("framed-production"));
    assert_eq!(first["outcome"], "committed");
    assert_eq!(first, replay);
    assert_eq!(first["exchange_id"], "exchange");
    assert_eq!(first["command_id"], "framed-command");
    assert_eq!(first["event_id"], "framed-event");
    assert_eq!(first["digest"].as_array().unwrap().len(), 32);
}

#[test]
fn native_writer_rejects_invalid_frames_without_opening_database() {
    fn frame(value: &serde_json::Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(value).unwrap();
        let mut result = (bytes.len() as u32).to_be_bytes().to_vec();
        result.extend(bytes);
        result
    }
    let valid = serde_json::json!({"version":1,"action":"apply","exchange_id":"exchange","command_id":"command","event_id":"event","operation":{"kind":"finish_pending","status":"done","inverse":null}});
    let mut cases = vec![
        (0u32.to_be_bytes().to_vec(), "invalid_frame"),
        ((1024 * 1024 + 1u32).to_be_bytes().to_vec(), "invalid_frame"),
        (vec![0, 0], "invalid_frame"),
        (vec![0, 0, 0, 8, b'{'], "invalid_frame"),
    ];
    for (field, value, expected) in [
        (
            "sql",
            serde_json::json!("DROP TABLE events"),
            "invalid_frame",
        ),
        ("status", serde_json::json!("pending"), "invalid_frame"),
        ("version", serde_json::json!(2), "invalid_command"),
        (
            "event_id",
            serde_json::json!("../outside"),
            "invalid_command",
        ),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        cases.push((frame(&invalid), expected));
    }
    let mut extra = frame(&valid);
    extra.push(0);
    cases.push((extra, "trailing_request"));
    for (bytes, code) in cases {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let reply = run_writer_with_frame(&root, Some("framed-finish"), Some(&bytes));
        assert_eq!(reply["outcome"], "rejected_before_open");
        assert_eq!(reply["code"], code);
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }
}

#[test]
fn native_writer_reports_uncertainty_after_database_attempt() {
    for case in ["absent", "missing-event", "receipt-failure"] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        if case != "absent" {
            let setup = crate::skill_event_store::EventStore::open(&root).unwrap();
            if case == "receipt-failure" {
                setup
                    .record(
                        "framed-event",
                        crate::skill_event::EventDraft {
                            kind: "fixture".into(),
                            skill: "fixture".into(),
                            harness: None,
                            scope: None,
                            project_path: None,
                            payload: serde_json::json!({}),
                            inverse: None,
                            backup_dir: None,
                            restorable: false,
                        },
                    )
                    .unwrap();
                setup.conn.execute_batch("CREATE TRIGGER refuse_receipt BEFORE INSERT ON event_command_receipts BEGIN SELECT RAISE(ABORT, 'private injected database error'); END;").unwrap();
            }
        }
        let reply = run_writer(&root, Some("framed-finish"));
        assert_eq!(reply["outcome"], "unknown");
        assert_eq!(reply["code"], "database_operation_failed");
        assert_eq!(reply["exchange_id"], "exchange");
        assert_eq!(reply["command_id"], "framed-command");
        assert_eq!(reply["event_id"], "framed-event");
        assert!(!reply.to_string().contains("private injected"));
        if case == "absent" {
            assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
        } else {
            let setup = crate::skill_event_store::EventStore::open(&root).unwrap();
            assert_eq!(
                setup
                    .conn
                    .query_row("SELECT count(*) FROM event_command_receipts", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            if case == "receipt-failure" {
                assert_eq!(
                    setup.get("framed-event").unwrap().unwrap().status,
                    "pending"
                );
                setup
                    .conn
                    .execute_batch("DROP TRIGGER refuse_receipt")
                    .unwrap();
                drop(setup);
                let retried = run_writer(&root, Some("framed-finish"));
                assert_eq!(retried["outcome"], "committed");
                assert_eq!(retried["digest"], reply["digest"]);
            }
        }
    }
}

#[test]
fn native_writer_recovers_committed_command_after_unusable_reply() {
    for mode in [
        "framed-lost-reply",
        "framed-partial-reply",
        "framed-wrong-reply",
        "framed-hold-exit",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let setup = crate::skill_event_store::EventStore::open(&root).unwrap();
        setup
            .record(
                "framed-event",
                crate::skill_event::EventDraft {
                    kind: "fixture".into(),
                    skill: "fixture".into(),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        drop(setup);
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let competing = || {
            CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
                &root,
                Some(std::time::Duration::from_millis(10)),
            )
            .unwrap()
        };
        let lease = competing()
            .acquire()
            .unwrap()
            .finalize_write(&scope, &[])
            .unwrap();
        let first = run_leased_writer(&root, lease, Some(mode), None);
        assert_eq!(first.exit.success(), mode != "framed-hold-exit");
        let uncertain = first.outcome.unwrap();
        assert!(competing().acquire().is_err());
        assert_eq!(uncertain["outcome"], "unknown");
        assert_eq!(uncertain["code"], "reply_unavailable");
        assert_eq!(uncertain["exchange_id"], "exchange");
        let second = run_leased_writer(&root, first.lease, Some("framed-finish"), None);
        assert!(second.exit.success());
        let recovered = second.outcome.unwrap();
        assert!(competing().acquire().is_err());
        assert_eq!(recovered["outcome"], "committed");
        assert_eq!(uncertain["digest"], recovered["digest"]);
        drop(second.lease);
        assert!(competing().acquire().is_ok());
        let store = crate::skill_event_store::EventStore::open(&root).unwrap();
        assert_eq!(store.get("framed-event").unwrap().unwrap().status, "done");
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM event_command_receipts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn native_cancellation_returns_lease_after_confirmed_cleanup() {
    use crate::skill_coordination::{
        CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect,
    };
    for partial in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let competing = || {
            CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
                &root,
                Some(std::time::Duration::from_millis(10)),
            )
            .unwrap()
        };
        let lease = competing()
            .acquire()
            .unwrap()
            .finalize_write(&scope, &[])
            .unwrap();
        let cancellation = CancellationToken::default();
        if !partial {
            cancellation.cancel();
        }
        let stopped = run_leased_writer_controlled(
            &root,
            lease,
            Some(if partial {
                "framed-cancel-prefix"
            } else {
                "framed-finish"
            }),
            None,
            &cancellation,
        );
        assert!(!stopped.exit.success());
        stopped.lease.validate_state_tree(&root).unwrap();
        let outcome = stopped.outcome.unwrap();
        assert_eq!(
            outcome["outcome"],
            if partial { "unknown" } else { "not_started" }
        );
        assert!(competing().acquire().is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        drop(stopped.lease);
        assert!(competing().acquire().is_ok());
    }
}

#[test]
fn native_writer_limits_permission_changes_to_database_inheritance() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    assert_eq!(
        run_writer(&root, Some("permissions"))["permission_policy"],
        true
    );
}

#[test]
fn native_writer_authorizes_fcntl_by_descriptor_and_role() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    assert_eq!(run_writer(&root, Some("fcntl"))["fcntl_policy"], true);
}

#[test]
fn native_sync_failure_reports_uncertainty_until_receipt_lookup() {
    use crate::skill_coordination::{
        CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect,
    };
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let store = crate::skill_event_store::EventStore::open(&root).unwrap();
    store
        .record(
            "sync-event",
            crate::skill_event::EventDraft {
                kind: "fixture".into(),
                skill: "fixture".into(),
                harness: None,
                scope: None,
                project_path: None,
                payload: serde_json::json!({}),
                inverse: None,
                backup_dir: None,
                restorable: false,
            },
        )
        .unwrap();
    drop(store);
    let request = crate::skill_event_worker_protocol::prepare_event_request(serde_json::from_value(serde_json::json!({"version":1,"action":"apply","exchange_id":"sync-apply","command_id":"sync-command","event_id":"sync-event","operation":{"kind":"finish_pending","status":"done","inverse":null}})).unwrap()).unwrap();
    let lookup = request.receipt_lookup("sync-lookup").unwrap();
    let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
    let lease = CoordinationPlan::new(
        vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
        None,
    )
    .unwrap()
    .acquire()
    .unwrap()
    .finalize_write(&scope, &[])
    .unwrap();
    let first = run_leased_exchange(
        &root,
        lease,
        Some("framed-sync-failure"),
        None,
        &CancellationToken::default(),
        Some(&request),
    );
    assert!(first.exit.success());
    assert_eq!(first.outcome.unwrap()["outcome"], "unknown");
    first.lease.validate_state_tree(&root).unwrap();
    let second = run_leased_exchange(
        &root,
        first.lease,
        Some("framed-finish"),
        None,
        &CancellationToken::default(),
        Some(&lookup),
    );
    assert!(second.exit.success());
    let outcome = second.outcome.unwrap();
    assert!(outcome["outcome"] == "receipt_absent" || outcome["outcome"] == "committed");
    drop(second.lease);
    let store = crate::skill_event_store::EventStore::open(&root).unwrap();
    assert_eq!(
        store
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    let committed = outcome["outcome"] == "committed";
    assert_eq!(
        store.get("sync-event").unwrap().unwrap().status,
        if committed { "done" } else { "pending" }
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM event_command_receipts", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        i64::from(committed)
    );
}

#[test]
fn native_writer_preserves_posix_methods_and_refuses_proxy_switching() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let report = run_writer(&root, Some("methods"));
    assert_eq!(report["proxy_switch_refused"], true);
    assert!(report["vfs"] == "unix-posix" || report["vfs"] == "unix");
}

#[test]
fn native_record_command_preserves_intent_replay_and_unresolved_guard() {
    use crate::skill_coordination::{
        CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect,
    };
    use crate::skill_event_worker_protocol::prepare_event_request;
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let store = crate::skill_event_store::EventStore::open(&root).unwrap();
    drop(store);
    let record = serde_json::json!({"version":1,"action":"apply","exchange_id":"record-first","command_id":"record-command","event_id":"record-event","operation":{"kind":"record_pending","timestamp":"2026-09-12T00:00:00Z","draft":{"kind":"fixture","skill":"fixture","harness":"codex","scope":"project","project_path":"/inert/project","payload":{"path":"/inert/payload"},"inverse":null,"backup_dir":"backups/fixture","restorable":false}}});
    let prepare = |value| prepare_event_request(serde_json::from_value(value).unwrap()).unwrap();
    let request = prepare(record.clone());
    let lookup = request.receipt_lookup("record-lookup").unwrap();
    let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
    let lease = CoordinationPlan::new(
        vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
        None,
    )
    .unwrap()
    .acquire()
    .unwrap()
    .finalize_write(&scope, &[])
    .unwrap();
    let run = |lease, request| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "skill_event_native::tests::native_writer_child",
                "--ignored",
                "--nocapture",
            ])
            .env("SKILL_STUDIO_EVENT_DESCRIPTOR_PROBE", "framed-production");
        crate::skill_event_worker_exchange::run_event_exchange(
            &root,
            lease,
            command,
            request,
            &CancellationToken::default(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .unwrap_or_else(|failure| panic!("{}", failure.reason))
    };
    let preflight = crate::skill_event_worker_protocol::PreparedEventExchange::preflight(
        "preflight".into(),
        "preflight-command".into(),
        "record-event".into(),
    )
    .unwrap();
    let ready = run(lease, &preflight);
    assert_eq!(ready.outcome.unwrap()["recovery_required"], false);
    let first = run(ready.lease, &request);
    assert!(first.exit.success());
    let receipt = first.outcome.unwrap();
    assert_eq!(receipt["outcome"], "committed");
    let pending = run(first.lease, &preflight);
    assert_eq!(pending.outcome.unwrap()["recovery_required"], true);
    let found = run(pending.lease, &lookup);
    assert!(found.exit.success());
    assert_eq!(found.outcome.unwrap()["digest"], receipt["digest"]);
    let mut other = record.clone();
    other["command_id"] = serde_json::json!("other-command");
    other["event_id"] = serde_json::json!("other-event");
    let other = prepare(other);
    let blocked = run(found.lease, &other);
    assert!(blocked.exit.success());
    assert_eq!(blocked.outcome.unwrap()["outcome"], "unknown");
    let snapshot = {
        let store = crate::skill_event_store::EventStore::open(&root).unwrap();
        store
            .conn
            .execute(
                "UPDATE events SET status='interrupted' WHERE id='record-event'",
                [],
            )
            .unwrap();
        store.get("record-event").unwrap().unwrap()
    };
    let finish = crate::skill_event_worker_protocol::PreparedEventExchange::finish_recovery(
        "finish".into(),
        "finish-command".into(),
        snapshot,
        crate::skill_event::EventStatus::Done,
        Some(serde_json::json!({"restore":"content"})),
    )
    .unwrap();
    let finished = run(blocked.lease, &finish);
    assert!(finished.exit.success());
    assert_eq!(finished.outcome.unwrap()["outcome"], "committed");
    let ready = run(finished.lease, &preflight);
    assert_eq!(ready.outcome.unwrap()["recovery_required"], false);
    let replayed = run(ready.lease, &request);
    assert!(replayed.exit.success());
    assert_eq!(replayed.outcome.unwrap()["digest"], receipt["digest"]);
    let mut changed = record;
    changed["operation"]["timestamp"] = serde_json::json!("2026-09-13T00:00:00Z");
    let changed = prepare(changed);
    let refused = run(replayed.lease, &changed);
    assert!(refused.exit.success());
    assert_eq!(refused.outcome.unwrap()["outcome"], "unknown");
    drop(refused.lease);
    let store = crate::skill_event_store::EventStore::open(&root).unwrap();
    let state: (String, String) = store
        .conn
        .query_row(
            "SELECT status, ts FROM events WHERE id='record-event'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, ("done".into(), "2026-09-12T00:00:00Z".into()));
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM event_command_receipts", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let row = store.get("record-event").unwrap().unwrap();
    assert_eq!(row.kind, "fixture");
    assert_eq!(row.skill, "fixture");
    assert_eq!(row.harness.as_deref(), Some("codex"));
    assert_eq!(row.scope.as_deref(), Some("project"));
    assert_eq!(row.project_path.as_deref(), Some("/inert/project"));
    assert_eq!(row.payload, serde_json::json!({"path":"/inert/payload"}));
    assert_eq!(row.backup_dir.as_deref(), Some("backups/fixture"));
    assert!(!row.restorable);
    assert_eq!(row.inverse, Some(serde_json::json!({"restore":"content"})));
    assert!(!root.join("backups").exists());
}

#[test]
fn native_initialization_creates_migrates_replays_and_refuses_orphans() {
    use crate::{
        skill_coordination::{
            CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect,
        },
        skill_event_worker_protocol::PreparedEventExchange,
    };
    for case in [
        "empty",
        "legacy",
        "events.sqlite3-wal",
        "events.sqlite3-shm",
        "events.sqlite3-journal",
    ] {
        let orphan = if matches!(case, "empty" | "legacy") {
            None
        } else {
            Some(case)
        };
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        if let Some(name) = orphan {
            std::fs::write(root.join(name), b"orphan fixture").unwrap();
        }
        if case == "legacy" {
            let legacy = Connection::open(root.join("events.sqlite3")).unwrap();
            legacy.execute_batch("CREATE TABLE events(id TEXT PRIMARY KEY, ts TEXT NOT NULL, kind TEXT NOT NULL, skill TEXT NOT NULL, harness TEXT, scope TEXT, project_path TEXT, payload TEXT NOT NULL, inverse TEXT, status TEXT NOT NULL, reverted_by TEXT); INSERT INTO events(id,ts,kind,skill,payload,status) VALUES('legacy','2026-01-01T00:00:00Z','fixture','legacy','{}','done');").unwrap();
        }
        let scope = SkillReadScope::bind(std::slice::from_ref(&root)).unwrap();
        let lease = CoordinationPlan::new(
            vec![DirectoryEffect::tree(&root, CoordinationMode::Exclusive)],
            None,
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&scope, &[])
        .unwrap();
        let run = |lease, request| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "skill_event_native::tests::native_writer_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("SKILL_STUDIO_EVENT_DESCRIPTOR_PROBE", "framed-production");
            crate::skill_event_worker_exchange::run_event_exchange(
                &root,
                lease,
                command,
                request,
                &CancellationToken::default(),
                std::time::Instant::now() + std::time::Duration::from_secs(5),
            )
            .unwrap_or_else(|failure| panic!("{}", failure.reason))
        };
        let preflight = PreparedEventExchange::preflight(
            "preflight".into(),
            "check".into(),
            "operation".into(),
        )
        .unwrap();
        let missing = run(lease, &preflight);
        assert_eq!(missing.outcome.unwrap()["outcome"], "unknown");
        assert_eq!(root.join("events.sqlite3").exists(), case == "legacy");
        let initialize = PreparedEventExchange::initialize(
            "initialize".into(),
            "initialize-command".into(),
            "operation".into(),
        )
        .unwrap();
        let first = run(missing.lease, &initialize);
        let reply = first.outcome.unwrap();
        if let Some(name) = orphan {
            assert_eq!(reply["outcome"], "unknown");
            assert!(!root.join("events.sqlite3").exists());
            assert_eq!(std::fs::read(root.join(name)).unwrap(), b"orphan fixture");
            continue;
        }
        assert_eq!(reply["outcome"], "committed");
        let replay = run(first.lease, &initialize);
        assert_eq!(replay.outcome.unwrap(), reply);
        let ready = run(replay.lease, &preflight);
        assert_eq!(ready.outcome.unwrap()["recovery_required"], false);
        drop(ready.lease);
        let store = crate::skill_event_store::EventStore::open(&root).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            if case == "legacy" { 1 } else { 0 }
        );
        if case == "legacy" {
            let event = store.get("legacy").unwrap().unwrap();
            assert_eq!(event.skill, "legacy");
            assert_eq!(event.status, "done");
            assert_eq!(event.ts, "2026-01-01T00:00:00Z");
            assert!(event.restorable);
            assert_eq!(event.backup_dir, None);
        }
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM event_command_receipts", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
