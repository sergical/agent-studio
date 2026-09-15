import { z } from "zod";

export const METHOD_LABEL = z
  .enum(["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "OTHER"])
  .catch("OTHER");
export const ROUTE_LABEL = z
  .enum([
    "/health",
    "/api/v1/skills",
    "/api/v1/skills/search",
    "/api/v1/skills/:owner/:repo/:slug",
    "unmatched",
  ])
  .catch("unmatched");

export function routeLabel(route: string): string {
  return ROUTE_LABEL.parse(route);
}

export function methodLabel(method: string): string {
  return METHOD_LABEL.parse(method);
}
