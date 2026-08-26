// ============================================================================
// PacksView - Share packs list + detail. A pack bundles selected skills into
// a dotagents-compatible repo under ~/.agents/packs/<name> - see
// `skill_pack.rs` and docs/agent-skill-conventions.md's "Packs" section.
// ============================================================================

import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { ChevronLeft, ExternalLink, Package } from "lucide-react";
import {
  deleteSkillPack,
  listSkillPacks,
  publishSkillPack,
  updateSkillPack,
} from "../../lib/skill-api";
import type { PackInfo } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";

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
    <div className="packs-detail">
      <button className="packs-detail-back" onClick={onBack}>
        <ChevronLeft size={14} />
        Packs
      </button>

      <h2>{pack.name}</h2>
      <p className="packs-detail-dir">{pack.dir}</p>
      {pack.repo ? (
        <a
          className="packs-detail-repo"
          href={`https://github.com/${pack.repo}`}
          target="_blank"
          rel="noreferrer"
        >
          {pack.repo}
          <ExternalLink size={12} />
        </a>
      ) : (
        <p className="packs-detail-repo packs-detail-repo-none">Local only - not published</p>
      )}

      <div className="packs-detail-skills">
        {pack.skills.map((name) => (
          <span key={name} className="packs-detail-skill-chip">
            {name}
          </span>
        ))}
      </div>

      <div className="packs-detail-actions">
        <button disabled={busy !== null} onClick={handleUpdate}>
          {busy === "update" ? "Updating…" : "Update pack"}
        </button>
        <button disabled={busy !== null} onClick={handlePublish}>
          {busy === "publish" ? "Publishing…" : pack.repo ? "Push update" : "Publish to GitHub"}
        </button>
        <button className="packs-detail-delete" disabled={busy !== null} onClick={handleDelete}>
          {busy === "delete" ? "Deleting…" : "Delete"}
        </button>
      </div>
    </div>
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
    <div className="packs-view">
      <div className="packs-view-header">
        <h2>Packs</h2>
        <span className="skill-store-count">
          {packs?.length ?? 0} pack{(packs?.length ?? 0) !== 1 ? "s" : ""}
        </span>
      </div>

      {packs === null ? (
        <p className="packs-view-empty">Loading…</p>
      ) : packs.length === 0 ? (
        <p className="packs-view-empty">
          No packs yet. Select skills from any list and click "Create pack" to bundle them for
          sharing.
        </p>
      ) : (
        <div className="packs-view-list">
          {packs.map((pack) => (
            <button
              key={pack.name}
              className="packs-view-row"
              onClick={() => setOpenName(pack.name)}
            >
              <Package size={15} />
              <span className="packs-view-row-name">{pack.name}</span>
              <span className="packs-view-row-count">
                {pack.skills.length} skill{pack.skills.length !== 1 ? "s" : ""}
              </span>
              <span className="packs-view-row-repo">{pack.repo ?? "Local only"}</span>
              <span className="packs-view-row-date">{formatDate(pack.created_at)}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
