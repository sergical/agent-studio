import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";
import { z } from "zod";

const oxlintJsonOutputSchema = z.object({
	diagnostics: z.array(z.object({ code: z.string() })),
});

function internalVocabularyDiagnostics(source: string): number {
	const fixtureDirectory = mkdtempSync(join(tmpdir(), "anti-slop-internal-vocabulary-"));
	const fixturePath = join(fixtureDirectory, "fixture.tsx");
	const configPath = join(fixtureDirectory, "oxlint.json");
	writeFileSync(fixturePath, source);
	writeFileSync(
		configPath,
		JSON.stringify({
			categories: { correctness: "off" },
			jsPlugins: [
				{ name: "anti-slop", specifier: resolve("tools/oxlint/anti-slop/index.ts") },
			],
			rules: { "anti-slop/no-internal-vocabulary": "error" },
		}),
	);

	try {
		const result = spawnSync(
			resolve("node_modules/.bin/oxlint"),
			["--config", configPath, "--format", "json", fixturePath],
			{ encoding: "utf8", timeout: 10_000 },
		);
		if (result.error !== undefined) throw result.error;
		return oxlintJsonOutputSchema
			.parse(JSON.parse(result.stdout))
			.diagnostics.filter(({ code }) => code === "anti-slop(no-internal-vocabulary)").length;
	} finally {
		rmSync(fixtureDirectory, { recursive: true, force: true });
	}
}

describe("anti-slop no-internal-vocabulary", () => {
	it("flags developer words in Error messages, toast text, and JSX text", () => {
		expect(
			internalVocabularyDiagnostics(`
				export function show(addToast: (toast: object) => void) {
					const failure = new Error("No unambiguous mutable deployment here");
					addToast({ type: "error", title: "Lifecycle owner missing", message: \`Harness \${failure}\` });
					return <p>Manage each deployment in Locations.</p>;
				}
			`),
		).toBe(4);
	});

	it("allows plain words and ignores identifiers, props, and unrelated strings", () => {
		expect(
			internalVocabularyDiagnostics(`
				export function show(addToast: (toast: object) => void, deployment: string) {
					const note = "deployment in a variable string is not user text";
					addToast({ type: "error", title: "Couldn't remove", message: "Remove each copy in Locations." });
					return <p data-harness={deployment}>{note} Copies stay put.</p>;
				}
				export const failure = new Error("Remove each copy from Locations.");
			`),
		).toBe(0);
	});

	it("ignores identifier-like codes with no spaces, even in user-text positions", () => {
		expect(
			internalVocabularyDiagnostics(`
				export const reasonLabel = "harness-unsupported";
				export const step = { message: "materialize_skill_root", title: "dep:v1/global/harness" };
				export function show() {
					return <Panel label="harness-unsupported" />;
				}
			`),
		).toBe(0);
	});

	it("reports one-word text that ends in punctuation and skips real codes", () => {
		expect(
			internalVocabularyDiagnostics(`
				export function show() {
					return <p>Harness:</p>;
				}
			`),
		).toBe(1);
		expect(
			internalVocabularyDiagnostics(`
				export const reasonLabel = "skill-name";
				export const stepText = "a.b";
			`),
		).toBe(0);
	});

	it("flags developer words in user-text props, JSX children, object keys, and text-returning functions", () => {
		expect(
			internalVocabularyDiagnostics(`
				export function show(flag: boolean, name: string) {
					const items = [{ label: "Split into harness folders", action: 1 }];
					const palette = { "title": "Open deployment", tooltipText: \`Canonical \${name}\` };
					return (
						<section aria-label="Filter by harness" emptyMessage="No deployment">
							<Panel label={flag ? "Harness" : "Copy"} />
							<p>{flag ? "All harnesses" : name}</p>
							<p>{flag && "Mutable copy"}</p>
						</section>
					);
				}
				function statusLabel(flag: boolean) {
					return flag ? "No harness" : "Ready";
				}
				const parkReason = (name: string) => \`\${name} is a deployment\`;
			`),
		).toBe(10);
	});

	it("ignores identifiers, className, console text, and keys that are not user text", () => {
		expect(
			internalVocabularyDiagnostics(`
				export function show(flag: boolean, harness: string) {
					console.info("harness deployment ready");
					const row = { kind: "harness_disable", harness: "deployment", id: "canonical" };
					const harnessLabel = harness;
					function pickKind() {
						return "harness";
					}
					return (
						<section className="harness-row" data-kind="deployment" aria-label="Filter by agent">
							<Panel label={harnessLabel} variant={flag ? "harness" : "deployment"} />
							<p>{row.kind}{pickKind()}</p>
						</section>
					);
				}
			`),
		).toBe(0);
	});
});
