// Pins plan.md section 7's lint set as it's wired into `npm run check` and
// `.oxlintrc.json`: knip (dead-code detection), the no-restricted-imports
// layering rule, and the vitest test step. A future edit that drops one of
// these from the check pipeline fails here by naming exactly what's
// missing, instead of silently shipping a weaker gate.

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

interface PackageJsonScripts {
	readonly scripts?: Readonly<Record<string, string>>;
}

function readRootPackageJsonScripts(): Readonly<Record<string, string>> {
	const raw = readFileSync("package.json", "utf-8");
	// SAFETY: this reads the workspace's own checked-in root package.json,
	// not external input; the assertion just narrows JSON.parse's `unknown`.
	const parsed = JSON.parse(raw) as PackageJsonScripts;
	return parsed.scripts ?? {};
}

describe("npm run check runs knip, no-restricted-imports, and vitest, or names the missing step", () => {
	it("npm_run_check_runs_knip_no_restricted_imports_and_vitest_or_names_the_missing_step", () => {
		const scripts = readRootPackageJsonScripts();
		const checkScript = scripts.check;
		expect(checkScript, "root package.json has no \"check\" script").toBeDefined();

		const missing: string[] = [];
		if (!checkScript?.includes("knip")) {
			missing.push('"check" does not run knip (dead-code detection)');
		}
		// The vitest gate is the root "test" script; "typecheck" and
		// "types:check" also contain the substring, so match the step itself.
		if (!/(^|&&)\s*npm run test(\s*(&&|$))/.test(checkScript ?? "")) {
			missing.push('"check" does not run the vitest test step');
		}

		const oxlintConfig = readFileSync(".oxlintrc.json", "utf-8");
		if (!oxlintConfig.includes("no-restricted-imports")) {
			missing.push(".oxlintrc.json has no no-restricted-imports layering rule");
		}

		expect(missing, missing.join("\n")).toEqual([]);
	});
});
