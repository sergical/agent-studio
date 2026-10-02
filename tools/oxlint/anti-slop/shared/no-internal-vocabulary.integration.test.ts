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
});
