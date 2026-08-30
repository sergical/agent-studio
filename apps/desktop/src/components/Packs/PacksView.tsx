// ============================================================================
// PacksView - Share packs list + detail. A pack bundles selected skills into
// a dotagents-compatible repo under ~/.agents/packs/<name> - see
// `skill_pack.rs` and docs/agent-skill-conventions.md's "Packs" section.
// ============================================================================

import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { ChevronLeft, ExternalLink, Package } from "lucide-react";
import { Button } from "@skill-studio/ui";
import {
  deleteSkillPack,
  listSkillPacks,
  publishSkillPack,
  updateSkillPack,
} from "../../lib/skill-api";
import type { PackInfo } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { PageShell } from "../Shell/PageShell";

function formatDate(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleDateString();
}

/** One pack's detail page: skill list, dir path, Update / Publish / Delete. */
function PackDetail({
  pack,
  onBack,
  onChanged,
}: {
  pack: PackInfo;
  onBack: () => void;
  onChanged: (pack: PackInfo | null) => void;
}) {
  const addToast = useAppStore((state) => state.addToast);
  const [busy, setBusy] = useState<"update" | "publish" | "delete" | null>(null);

  async function handleUpdate() {
    setBusy("update");
    try {
      const result = await updateSkillPack(pack.name);
      onChanged(result.pack);
      addToast({
        type: "success",
        title: result.changed ? "Pack updated" : "Already up to date",
        message: pack.name,
      });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't update pack",
        message: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setBusy(null);
    }
  }

  async function handlePublish() {
    // The confirm dialog itself runs on the backend, right before `gh`/`git`
    // - see `publish_skill_pack`'s `PublishConfirm`. A "Publish cancelled"
    // error means the user dismissed that dialog, not a real failure.
    setBusy("publish");
    try {
      const updated = await publishSkillPack(pack.name, "public");
      onChanged(updated);
      addToast({ type: "success", title: "Published", message: updated.repo ?? pack.name });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (message === "Publish cancelled") {
        addToast({ type: "info", title: "Publish cancelled", message: pack.name });
      } else {
        addToast({ type: "error", title: "Couldn't publish pack", message });
      }
    } finally {
      setBusy(null);
    }
  }

  async function handleDelete() {
    const confirmed = await ask(
      `Delete ${pack.name} locally? Its GitHub repo (if any) is left untouched.`,
      { title: "Delete pack", kind: "warning" },
    );
    if (!confirmed) return;

    setBusy("delete");
    try {
      await deleteSkillPack(pack.name);
      onChanged(null);
      onBack();
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't delete pack",
        message: err instanceof Error ? err.message : String(err),
      });
      setBusy(null);
    }
  }

  return (
    <PageShell
      title={pack.name}
      width="narrow"
      actions={
        <button
          className="inline-flex shrink-0 items-center gap-1 self-start border-0 bg-none p-0 text-small text-text-tertiary"
          onClick={onBack}
        >
          <ChevronLeft size={14} />
          Packs
        </button>
      }
    >
      <div className="flex max-w-140 flex-col gap-2.5">
        <p className="m-0 text-small text-text-tertiary">{pack.dir}</p>
        {pack.repo ? (
          <a
            className="inline-flex w-fit items-center gap-1 text-small text-accent"
            href={`https://github.com/${pack.repo}`}
            target="_blank"
            rel="noreferrer"
          >
            {pack.repo}
            <ExternalLink size={12} />
          </a>
        ) : (
          <p className="m-0 text-small text-text-tertiary">Local only - not published</p>
        )}

        <div className="mt-1 flex flex-wrap gap-1.5">
          {pack.skills.map((name) => (
            <span
              key={name}
              className="rounded-sm bg-bg-tertiary px-2 py-0.75 text-caption text-text-secondary"
            >
              {name}
            </span>
          ))}
        </div>

        <div className="mt-3 flex gap-2">
          <Button variant="outline" disabled={busy !== null} onClick={handleUpdate}>
            {busy === "update" ? "Updating…" : "Update pack"}
          </Button>
          <Button variant="outline" disabled={busy !== null} onClick={handlePublish}>
            {busy === "publish" ? "Publishing…" : pack.repo ? "Push update" : "Publish to GitHub"}
          </Button>
          <Button variant="destructive" disabled={busy !== null} onClick={handleDelete}>
            {busy === "delete" ? "Deleting…" : "Delete"}
          </Button>
        </div>
      </div>
    </PageShell>
  );
}

/** Every pack, sorted by creation time (newest first), plus its detail page. */
export function PacksView() {
  const [packs, setPacks] = useState<PackInfo[] | null>(null);
  const [openName, setOpenName] = useState<string | null>(null);
  const addToast = useAppStore((state) => state.addToast);

  useEffect(() => {
    let cancelled = false;
    listSkillPacks()
      .then((result) => {
        if (!cancelled) setPacks(result);
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't load packs",
          message: err instanceof Error ? err.message : String(err),
        });
      });
    return () => {
      cancelled = true;
    };
  }, [addToast]);

  const openPack = packs?.find((p) => p.name === openName) ?? null;

  function handleChanged(updated: PackInfo | null) {
    setPacks((prev) => {
      if (!prev) return prev;
      if (updated === null) return prev.filter((p) => p.name !== openName);
      return prev.map((p) => (p.name === updated.name ? updated : p));
    });
  }

  if (openPack) {
    return (
      <PackDetail
        pack={openPack}
        onBack={() => setOpenName(null)}
        onChanged={(updated) => {
          handleChanged(updated);
          if (updated === null) setOpenName(null);
        }}
      />
    );
  }

  return (
    <PageShell
      title="Packs"
      actions={
        <span className="text-small text-text-tertiary tabular-nums">
          {packs?.length ?? 0} pack{(packs?.length ?? 0) !== 1 ? "s" : ""}
        </span>
      }
    >
      {packs === null ? (
        <p className="m-0 text-wrap-pretty text-small text-text-tertiary">Loading…</p>
      ) : packs.length === 0 ? (
        <p className="m-0 text-wrap-pretty text-small text-text-tertiary">
          No packs yet. Select skills from any list and click "Create pack" to bundle them for
          sharing.
        </p>
      ) : (
        <div className="flex flex-col gap-1.5">
          {packs.map((pack) => (
            <button
              key={pack.name}
              className="flex h-11 w-full items-center gap-2.5 rounded-md border border-border bg-bg-secondary px-3 text-left text-text-secondary transition-colors hover:bg-bg-hover"
              onClick={() => setOpenName(pack.name)}
            >
              <Package size={15} />
              <span className="text-body font-semibold text-text-primary">{pack.name}</span>
              <span className="text-caption text-text-tertiary">
                {pack.skills.length} skill{pack.skills.length !== 1 ? "s" : ""}
              </span>
              <span className="ml-auto text-caption text-text-tertiary">
                {pack.repo ?? "Local only"}
              </span>
              <span className="text-caption text-text-tertiary">{formatDate(pack.created_at)}</span>
            </button>
          ))}
        </div>
      )}
    </PageShell>
  );
}
