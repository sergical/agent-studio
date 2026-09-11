// ============================================================================
// Skill Studio TUI - App shell
// Owns the screen state machine (inventory/issues tabs, detail overlay), the
// initial `scan`, the lazy `diagnose`, and the `watch` subscription. Every
// process concern is delegated to `transport` (real `cliTransport` by
// default, a stub in tests) - this component never spawns anything itself.
// ============================================================================

import type { TabSelectOption } from "@opentui/core";
import { useKeyboard } from "@opentui/react";
import { useEffect, useState } from "react";

import {
  cliTransport,
  CliTransportError,
  type ScopeConfig,
  type Transport,
} from "./cli-transport.ts";
import type { Diagnosis, Inventory } from "./cli-types.ts";
import { StatusLine, type WatchStatus } from "./components/StatusLine.tsx";
import { DetailScreen } from "./screens/DetailScreen.tsx";
import { InventoryScreen } from "./screens/InventoryScreen.tsx";
import { IssuesScreen } from "./screens/IssuesScreen.tsx";

/** Stable identity across renders: a fresh `{}` literal as a default
 * parameter would re-trigger every effect keyed on `scopeConfig`. */
const EMPTY_SCOPE_CONFIG: ScopeConfig = {};

type Tab = "inventory" | "issues";

const TABS: TabSelectOption[] = [
  { name: "Inventory", description: "Skills by harness", value: "inventory" },
  { name: "Issues", description: "diagnose output", value: "issues" },
];

export interface AppProps {
  transport?: Transport;
  scopeConfig?: ScopeConfig;
  /** Called on `q`. Defaults to a no-op so component tests never exit the process. */
  onQuit?: () => void;
}

function asTransportError(cause: unknown): CliTransportError {
  if (cause instanceof CliTransportError) return cause;
  return new CliTransportError(
    "transport_error",
    cause instanceof Error ? cause.message : String(cause),
  );
}

export function App({
  transport = cliTransport,
  scopeConfig = EMPTY_SCOPE_CONFIG,
  onQuit = () => {},
}: AppProps) {
  const [tab, setTab] = useState<Tab>("inventory");
  const [inventory, setInventory] = useState<Inventory | null>(null);
  const [scanLoading, setScanLoading] = useState(true);
  const [scanError, setScanError] = useState<CliTransportError | null>(null);
  const [diagnosis, setDiagnosis] = useState<Diagnosis | null>(null);
  const [diagnosisLoading, setDiagnosisLoading] = useState(false);
  const [diagnosisError, setDiagnosisError] = useState<CliTransportError | null>(null);
  const [revision, setRevision] = useState<number | null>(null);
  const [lastRefreshedAt, setLastRefreshedAt] = useState<Date | null>(null);
  const [watchStatus, setWatchStatus] = useState<WatchStatus>({ kind: "connecting" });
  const [openSkillName, setOpenSkillName] = useState<string | null>(null);

  useKeyboard((key) => {
    if (key.name === "q") onQuit();
  });

  useEffect(() => {
    let cancelled = false;
    transport
      .run<Inventory>("scan", [], scopeConfig)
      .then((envelope) => {
        if (cancelled) return;
        if (envelope.data !== null) {
          setInventory(envelope.data);
          setLastRefreshedAt(new Date());
        }
        setScanLoading(false);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setScanError(asTransportError(cause));
        setScanLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [transport, scopeConfig]);

  useEffect(() => {
    const handle = transport.watch(
      undefined,
      (line) => {
        // The watch line already carries the full inventory for its
        // revision; applying it directly avoids racing a second `scan`
        // against the watcher (docs/spec-core-primitives.md 8.3, delta 1).
        setInventory(line.inventory);
        setRevision(line.revision);
        setLastRefreshedAt(new Date());
        setWatchStatus({ kind: "connected" });
      },
      (status) => {
        setWatchStatus(status);
      },
      scopeConfig,
    );
    return () => handle.stop();
  }, [transport, scopeConfig]);

  useEffect(() => {
    if (tab !== "issues") return;
    let cancelled = false;
    setDiagnosisLoading(true);
    setDiagnosisError(null);
    transport
      .run<Diagnosis>("diagnose", [], scopeConfig)
      .then((envelope) => {
        if (cancelled) return;
        setDiagnosis(envelope.data);
        setDiagnosisLoading(false);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setDiagnosisError(asTransportError(cause));
        setDiagnosisLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [tab, transport, scopeConfig]);

  const openSkill =
    openSkillName === null
      ? null
      : (inventory?.skills.find((s) => s.name === openSkillName) ?? null);

  return (
    <box style={{ flexDirection: "column", flexGrow: 1 }}>
      <tab-select
        options={TABS}
        onSelect={(_index, option) => {
          if (option !== null && (option.value === "inventory" || option.value === "issues")) {
            setTab(option.value);
          }
        }}
      />
      <box style={{ flexGrow: 1 }}>
        {openSkill !== null ? (
          <DetailScreen skill={openSkill} onBack={() => setOpenSkillName(null)} />
        ) : scanError !== null ? (
          <text>{`error (${scanError.code}): ${scanError.message}`}</text>
        ) : scanLoading || inventory === null ? (
          <text>Loading inventory…</text>
        ) : tab === "inventory" ? (
          <InventoryScreen inventory={inventory} onOpenSkill={setOpenSkillName} />
        ) : (
          <IssuesScreen
            diagnosis={diagnosis}
            loading={diagnosisLoading}
            error={diagnosisError}
            onOpenSkill={setOpenSkillName}
          />
        )}
      </box>
      <StatusLine revision={revision} lastRefreshedAt={lastRefreshedAt} watchStatus={watchStatus} />
    </box>
  );
}
