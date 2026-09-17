import type { SkillEvent } from "@skill-studio/lib";

export class SkillHistoryReader {
  private pending = new Map<string, Promise<SkillEvent[]>>();

  constructor(private readonly fetch: (limit?: number, skill?: string) => Promise<SkillEvent[]>) {}

  read(limit = 200, skill?: string): Promise<SkillEvent[]> {
    const key = JSON.stringify([limit, skill ?? null]);
    const existing = this.pending.get(key);
    if (existing) return existing;
    const request = Promise.resolve()
      .then(() => this.fetch(limit, skill))
      .finally(() => {
        if (this.pending.get(key) === request) this.pending.delete(key);
      });
    this.pending.set(key, request);
    return request;
  }

  invalidate(): void {
    this.pending.clear();
  }
}
