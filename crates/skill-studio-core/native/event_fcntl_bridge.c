#include "event_fcntl_bridge.h"
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stddef.h>

static skill_studio_event_fd_authorizer authorize_fd;

void skill_studio_event_set_fd_authorizer(skill_studio_event_fd_authorizer authorize) {
    authorize_fd = authorize;
}

static int denied(void) {
    errno = EACCES;
    return -1;
}

int skill_studio_event_fcntl(int descriptor, int command, ...) {
    if (authorize_fd == NULL) {
        return denied();
    }
    switch (command) {
        case F_GETLK:
        case F_SETLK:
        case F_SETLKW: {
            va_list arguments;
            va_start(arguments, command);
            struct flock *lock = va_arg(arguments, struct flock *);
            va_end(arguments);
            if (lock == NULL) {
                errno = EINVAL;
                return -1;
            }
            if (!authorize_fd(descriptor, command, lock, 0)) { return denied(); }
            return fcntl(descriptor, command, lock);
        }
        case F_GETFD:
            if (!authorize_fd(descriptor, command, NULL, 0)) { return denied(); }
            return fcntl(descriptor, command);
        case F_SETFD: {
            va_list arguments;
            va_start(arguments, command);
            int flags = va_arg(arguments, int);
            va_end(arguments);
            if (flags != FD_CLOEXEC || !authorize_fd(descriptor, command, NULL, flags)) {
                return denied();
            }
            return fcntl(descriptor, command, flags);
        }
#ifdef F_FULLFSYNC
        case F_FULLFSYNC:
            if (!authorize_fd(descriptor, command, NULL, 0)) { return denied(); }
            return fcntl(descriptor, command, 0);
#endif
        default:
            /* Unknown commands must not cause an argument of an unknown type to be read. */
            return denied();
    }
}
