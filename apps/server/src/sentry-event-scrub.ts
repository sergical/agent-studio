// ============================================================================
// Skill Studio - Sentry Event Scrub
// The Worker's Sentry `beforeSend` hook: this proxy is public, so an event it
// sends must never leak who called it or what they asked for.
// ============================================================================

import type { ErrorEvent } from "@sentry/cloudflare";

/** Strips request headers, cookies, query strings, and user data from a Sentry event before it leaves the Worker,
 *  so a caller's IP (cf-connecting-ip), Authorization header, or search text never reach Sentry. */
export function scrubSentryEvent<E extends ErrorEvent>(event: E): E {
  delete event.user;
  if (event.request) {
    delete event.request.headers;
    delete event.request.cookies;
    delete event.request.query_string;
    delete event.request.data;
    if (event.request.url) {
      event.request.url = event.request.url.split("?")[0];
    }
  }
  return event;
}
