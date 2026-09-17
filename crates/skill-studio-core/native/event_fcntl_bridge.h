#ifndef SKILL_STUDIO_EVENT_FCNTL_BRIDGE_H
#define SKILL_STUDIO_EVENT_FCNTL_BRIDGE_H

struct flock;
typedef int (*skill_studio_event_fd_authorizer)(int descriptor, int command, const struct flock *lock, int flags);

/* Install once in the private worker before SQLite can invoke the bridge. */
void skill_studio_event_set_fd_authorizer(skill_studio_event_fd_authorizer authorize);
int skill_studio_event_fcntl(int descriptor, int command, ...);

#endif
