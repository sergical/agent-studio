import { z } from "zod";
import type { SkillRefreshPosition, SkillSnapshot } from "@skill-studio/lib";

const refreshPosition = z.object({
  instance_id: z.string().min(1).max(64),
  generation: z.string().regex(/^(0|[1-9][0-9]{0,19})$/),
});

function generation(position: SkillRefreshPosition): bigint | undefined {
  const parsed = refreshPosition.safeParse(position);
  if (!parsed.success) return undefined;
  const value = BigInt(parsed.data.generation);
  return value < 18446744073709551615n ? value : undefined;
}

export function snapshotCoversRefresh(
  snapshot: SkillSnapshot | undefined,
  receipt: SkillRefreshPosition,
): boolean {
  const coverage = snapshot?.full_refresh;
  if (!coverage || coverage.instance_id !== receipt.instance_id) return false;
  const required = generation(receipt);
  const covered = generation(coverage);
  return required !== undefined && covered !== undefined && covered >= required;
}

interface RefreshDependencies {
  request: () => Promise<SkillRefreshPosition>;
  read: () => Promise<SkillSnapshot | undefined>;
  publish: (snapshot: SkillSnapshot) => void;
  timeoutMs?: number;
}
interface PendingRefresh {
  promise: Promise<void>;
  resolve: () => void;
  reject: (error: Error) => void;
  receipt?: SkillRefreshPosition;
  deadline: ReturnType<typeof setTimeout>;
  catchUp: ReturnType<typeof setTimeout>;
}

export class SkillRefreshWaiter {
  private pending?: PendingRefresh;
  private latest?: SkillSnapshot;
  private disposed = false;
  private reading = false;

  constructor(private readonly dependencies: RefreshDependencies) {}

  accept(snapshot: SkillSnapshot): void {
    if (this.disposed) return;
    if (!this.latest || snapshot.revision > this.latest.revision) this.latest = snapshot;
    if (this.pending?.receipt && snapshotCoversRefresh(snapshot, this.pending.receipt))
      this.finish();
  }

  request(): Promise<void> {
    if (this.disposed) return Promise.reject(new Error("Refresh wait cancelled"));
    if (this.pending) return this.pending.promise;
    let resolve: () => void = () => undefined;
    let reject: (error: Error) => void = () => undefined;
    const promise = new Promise<void>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    const timeout = this.dependencies.timeoutMs ?? 120_000;
    const pending: PendingRefresh = {
      promise,
      resolve,
      reject,
      deadline: setTimeout(
        () =>
          this.finish(
            new Error("Refresh is taking longer than expected. It may still be running."),
          ),
        timeout,
      ),
      catchUp: setTimeout(() => void this.catchUp(), Math.max(0, timeout - 1000)),
    };
    this.pending = pending;
    void Promise.resolve()
      .then(() => this.dependencies.request())
      .then((receipt) => {
        if (this.pending !== pending) return;
        if (generation(receipt) === undefined) {
          this.finish(new Error("Invalid refresh receipt"));
          return;
        }
        pending.receipt = receipt;
        if (snapshotCoversRefresh(this.latest, receipt)) this.finish();
        else void this.catchUp();
      })
      .catch(() => {
        if (this.pending === pending) this.finish(new Error("Failed to request refresh"));
      });
    return promise;
  }

  dispose(): void {
    this.disposed = true;
    this.latest = undefined;
    this.finish(new Error("Refresh wait cancelled"));
  }

  private finish(error?: Error): void {
    const pending = this.pending;
    if (!pending) return;
    this.pending = undefined;
    clearTimeout(pending.deadline);
    clearTimeout(pending.catchUp);
    if (error) pending.reject(error);
    else pending.resolve();
  }

  private async catchUp(): Promise<void> {
    if (this.disposed || !this.pending?.receipt || this.reading) return;
    this.reading = true;
    try {
      const snapshot = await this.dependencies.read();
      if (snapshot && !this.disposed) {
        this.dependencies.publish(snapshot);
        this.accept(snapshot);
      }
    } catch {
      // A snapshot event can still complete the wait after a catch-up read fails.
    } finally {
      this.reading = false;
    }
  }
}
