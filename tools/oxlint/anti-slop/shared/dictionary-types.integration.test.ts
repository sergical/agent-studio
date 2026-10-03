import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";
import { z } from "zod";

interface OxlintJsonDiagnostic {
	readonly code: string;
	readonly message: string;
}

const oxlintJsonOutputSchema = z.object({
	diagnostics: z.array(z.object({ code: z.string(), message: z.string() })),
});

function parseOxlintDiagnostics(stdout: string): readonly OxlintJsonDiagnostic[] {
	return oxlintJsonOutputSchema.parse(JSON.parse(stdout)).diagnostics;
}

function lintAntiSlopFixture(
	source: string,
	rules: readonly string[],
): readonly OxlintJsonDiagnostic[] {
	const fixtureDirectory = mkdtempSync(join(tmpdir(), "anti-slop-dictionary-types-"));
	const fixturePath = join(fixtureDirectory, "fixture.ts");
	const configPath = join(fixtureDirectory, "oxlint.json");
	writeFileSync(fixturePath, source);
	writeFileSync(
		configPath,
		JSON.stringify({
			categories: { correctness: "off" },
			jsPlugins: [
				{
					name: "anti-slop",
					specifier: resolve("tools/oxlint/anti-slop/index.ts"),
				},
			],
			rules: Object.fromEntries(rules.map((rule) => [`anti-slop/${rule}`, "error"])),
		}),
	);

	try {
		const result = spawnSync(
			resolve("node_modules/.bin/oxlint"),
			["--config", configPath, "--format", "json", fixturePath],
			{ encoding: "utf8", timeout: 10_000 },
		);
		if (result.error !== undefined) throw result.error;
		if (result.status !== 0 && result.status !== 1) {
			throw new Error(`Anti-slop Oxlint fixture failed: ${result.stderr || result.stdout}`);
		}
		return parseOxlintDiagnostics(result.stdout).filter((diagnostic) =>
			diagnostic.code.startsWith("anti-slop("),
		);
	} finally {
		rmSync(fixtureDirectory, { recursive: true, force: true });
	}
}

function diagnosticCount(diagnostics: readonly OxlintJsonDiagnostic[], code: string): number {
	return diagnostics.filter((diagnostic) => diagnostic.code === `anti-slop(${code})`).length;
}

describe("anti-slop interface dictionary rules", () => {
	it("binds unknown, any, and default generic interface arguments", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface GenericDictionary<Value> {
				[key: string]: Value;
			}
			interface DefaultDictionary<Value = unknown> {
				[key: string]: Value;
			}
			const unknownDictionary: GenericDictionary<unknown> = {};
			const anyDictionary: GenericDictionary<any> = {};
			const defaultDictionary: DefaultDictionary = {};
			const safeDictionary: GenericDictionary<string> = {};
			void unknownDictionary;
			void anyDictionary;
			void defaultDictionary;
			void safeDictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(3);
		expect(diagnostics.filter((diagnostic) => diagnostic.message.includes("unknown"))).toHaveLength(
			2,
		);
		expect(diagnostics.filter((diagnostic) => diagnostic.message.includes("any"))).toHaveLength(1);
	});

	it("flags known evidence assigned and asserted to a generic interface dictionary", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface GenericDictionary<Value> {
				[key: string]: Value;
			}
			interface OpenDictionary {
				[key: string]: string;
			}
			const assigned: GenericDictionary<unknown> = { known: "value" };
			const asserted = { known: "value" } as GenericDictionary<unknown>;
			const openDictionary: OpenDictionary = { known: "value" };
			void assigned;
			void asserted;
			void openDictionary;`,
			["no-known-value-widening"],
		);

		expect(diagnosticCount(diagnostics, "no-known-value-widening")).toBe(3);
		expect(
			diagnostics.filter((diagnostic) => diagnostic.message.includes("generic container")),
		).toHaveLength(2);
		expect(
			diagnostics.filter((diagnostic) => diagnostic.message.includes("open dictionary")),
		).toHaveLength(1);
	});

	it("classifies generic and plain alias wrappers around interface dictionaries", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface GenericDictionary<Value> {
				[key: string]: Value;
			}
			type GenericWrapper<Value> = GenericDictionary<Value>;
			type PlainWrapper = GenericDictionary<string>;
			const genericWrapper: GenericWrapper<unknown> = { known: "value" };
			const plainWrapper: PlainWrapper = { known: "value" };
			void genericWrapper;
			void plainWrapper;`,
			["no-unsafe-dictionary-type", "no-known-value-widening"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(diagnosticCount(diagnostics, "no-known-value-widening")).toBe(2);
		expect(
			diagnostics.filter((diagnostic) => diagnostic.message.includes("generic container")),
		).toHaveLength(1);
		expect(
			diagnostics.filter((diagnostic) => diagnostic.message.includes("open dictionary")),
		).toHaveLength(1);
	});

	it("collects substituted inherited and merged interface index signatures", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface BaseDictionary<Value> {
				[key: string]: Value;
			}
			interface InheritedDictionary<Payload> extends BaseDictionary<Payload> {}
			interface MergedDictionary<Value> {
				label?: string;
			}
			interface MergedDictionary<Value> {
				[key: string]: Value;
			}
			const inheritedUnsafe: InheritedDictionary<unknown> = {};
			const inheritedSafe: InheritedDictionary<string> = {};
			const mergedUnsafe: MergedDictionary<unknown> = {};
			void inheritedUnsafe;
			void inheritedSafe;
			void mergedUnsafe;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(2);
	});

	it("binds explicit heritage arguments in the caller scope", () => {
		const unsafeDiagnostics = lintAntiSlopFixture(
			`interface Base<A, B> {
				[key: string]: B;
			}
			interface Derived<A, B> extends Base<B, A> {}
			const dictionary: Derived<unknown, string> = {};
			void dictionary;`,
			["no-unsafe-dictionary-type"],
		);
		const safeDiagnostics = lintAntiSlopFixture(
			`interface Base<A, B> {
				[key: string]: B;
			}
			interface Derived<A, B> extends Base<B, A> {}
			const dictionary: Derived<string, unknown> = {};
			void dictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(unsafeDiagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(safeDiagnostics).toEqual([]);
	});

	it("keeps a caller Readonly parameter out of the base interface scope", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base<Value> {
				[key: string]: Readonly<Value>;
			}
			interface Derived<Readonly> extends Base<string> {}
			const dictionary: Derived<unknown> = {};
			void dictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnostics).toEqual([]);
	});

	it("keeps a caller parameter out of the base interface global-alias scope", () => {
		const diagnostics = lintAntiSlopFixture(
			`type GlobalValue = string;
			interface Base {
				[key: string]: GlobalValue;
			}
			interface Derived<GlobalValue> extends Base {}
			const dictionary: Derived<unknown> = {};
			void dictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnostics).toEqual([]);
	});

	it("preserves caller substitutions inside nested heritage arguments", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base<Value> {
				[key: string]: Value;
			}
			interface Derived<Value> extends Base<Readonly<Value>> {}
			const dictionary: Derived<unknown> = {};
			void dictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(diagnostics[0]?.message).toContain("unknown");
	});

	it("resolves same-named caller arguments through their saved scopes", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			type AliasDictionary<Value> = Record<string, Value>;
			interface InterfaceDictionary<Value> extends Record<string, Value> {}
			const aliasDictionary: AliasDictionary<Value> = {};
			const interfaceDictionary: InterfaceDictionary<Value> = {};
			void aliasDictionary;
			void interfaceDictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(2);
		expect(diagnostics.every((diagnostic) => diagnostic.message.includes("unknown"))).toBe(true);
	});

	it("keeps generic function parameters unresolved in nested lexical scopes", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			type OuterValue = unknown;
			interface Dictionary<Entry> {
				[key: string]: Entry;
			}
			function consumeDeclaration<Value>(input: Dictionary<Value>) {
				void input;
			}
			const consumeExpression = function <Value>(input: Dictionary<Value>) {
				void input;
			};
			const consumeArrow = <Value>(input: Dictionary<Value>) => void input;
			declare function consumeDeclared<Value>(input: Dictionary<Value>): void;
			function consumeOverload<Value>(input: Dictionary<Value>): void;
			function consumeOverload(input: Dictionary<string>): void {
				void input;
			}
			function consumeOuter<OuterValue>() {
				return <InnerValue>(input: Dictionary<OuterValue>) => void input;
			}
			function consumeBuiltIn<Record>(input: Dictionary<Record>) {
				void input;
			}
			void consumeDeclaration;
			void consumeExpression;
			void consumeArrow;
			void consumeDeclared;
			void consumeOverload;
			void consumeOuter;
			void consumeBuiltIn;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnostics).toEqual([]);
	});

	it("keeps generic class parameters unresolved in class declarations and expressions", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			type MethodValue = unknown;
			interface Dictionary<Entry> {
				[key: string]: Entry;
			}
			class DictionaryOwner<Value> {
				declare entries: Dictionary<Value>;
				consume(input: Dictionary<Value>) {
					void input;
				}
				consumeGeneric<MethodValue>(input: Dictionary<MethodValue>) {
					void input;
				}
			}
			const DictionaryOwnerExpression = class<Value> {
				declare entries: Dictionary<Value>;
			};
			void DictionaryOwner;
			void DictionaryOwnerExpression;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnostics).toEqual([]);
	});

	it("keeps signature type parameters unresolved on their owning AST nodes", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			interface Dictionary<Entry> {
				[key: string]: Entry;
			}
			interface GenericSignatures {
				<Value>(input: Dictionary<Value>): void;
				new <Value>(input: Dictionary<Value>): object;
				consume<Value>(input: Dictionary<Value>): void;
			}
			type GenericFunction = <Value>(input: Dictionary<Value>) => void;
			type GenericConstructor = new <Value>(input: Dictionary<Value>) => object;
			abstract class AbstractConsumer {
				abstract consume<Value>(input: Dictionary<Value>): void;
			}
			let signatures: GenericSignatures;
			let genericFunction: GenericFunction;
			let genericConstructor: GenericConstructor;
			void signatures;
			void genericFunction;
			void genericConstructor;
			void AbstractConsumer;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnostics).toEqual([]);
	});

	it("reports concrete alias instantiations outside generic lexical scopes", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			interface Dictionary<Entry> {
				[key: string]: Entry;
			}
			function consume<Value>(input: Dictionary<Value>) {
				void input;
			}
			const concreteDictionary: Dictionary<Value> = {};
			void consume;
			void concreteDictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(diagnostics[0]?.message).toContain("unknown");
	});

	it("keeps a caller PropertyKey parameter out of mapped alias widening scope", () => {
		const diagnostics = lintAntiSlopFixture(
			`type BaseDictionary = { [Key in PropertyKey]: string };
			type DerivedDictionary<PropertyKey> = BaseDictionary;
			type ConcreteDictionary = DerivedDictionary<"known">;
			const dictionary: ConcreteDictionary = { known: "value" };
			void dictionary;`,
			["no-known-value-widening"],
		);

		expect(diagnosticCount(diagnostics, "no-known-value-widening")).toBe(1);
		expect(diagnostics[0]?.message).toContain("open dictionary");
	});

	it("keeps a mapped key parameter out of its value's global alias scope", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Key = unknown;
			type SafeMappedDictionary = { [Key in "known"]: Key };
			const dictionary: SafeMappedDictionary = { known: "known" };
			void dictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnostics).toEqual([]);
	});

	it("binds mapped keys only in value and remapped-name branches", () => {
		const scopedDiagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			interface Dictionary<Entry> {
				[key: string]: Entry;
			}
			type MappedDictionary = { [Value in "known"]: Dictionary<Value> };
			type RemappedDictionary = {
				[Value in "known" as keyof Dictionary<Value>]: string;
			};
			const mappedDictionary: MappedDictionary = { known: {} };
			const remappedDictionary: RemappedDictionary = { known: "known" };
			void mappedDictionary;
			void remappedDictionary;`,
			["no-unsafe-dictionary-type"],
		);
		const unscopedDiagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			interface Dictionary<Entry> {
				[key: string]: Entry;
			}
			type MappedConstraint = { [Value in keyof Dictionary<Value>]: string };
			const concreteDictionary: Dictionary<Value> = {};
			void concreteDictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(scopedDiagnostics).toEqual([]);
		expect(diagnosticCount(unscopedDiagnostics, "no-unsafe-dictionary-type")).toBe(2);
		expect(
			unscopedDiagnostics.every((diagnostic) => diagnostic.message.includes("unknown")),
		).toBe(true);
	});

	it("limits conditional infer parameters to the true branch", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Value = unknown;
			interface Dictionary<Entry> {
				[key: string]: Entry;
			}
			type Direct<T> = T extends infer Value ? Dictionary<Value> : never;
			type Nested<T> = T extends Promise<infer Value>
				? Dictionary<Value>
				: Dictionary<Value>;
			const concreteDictionary: Dictionary<Value> = {};
			void concreteDictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(2);
		expect(diagnostics.every((diagnostic) => diagnostic.message.includes("unknown"))).toBe(true);
	});

	it("applies merged interface defaults to every declaration", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Dictionary<Value> {
				[key: string]: Value;
			}
			interface Dictionary<Value = unknown> {}
			const defaultDictionary: Dictionary = {};
			const explicitUnsafeDictionary: Dictionary<any> = {};
			const explicitSafeDictionary: Dictionary<string> = {};
			void defaultDictionary;
			void explicitUnsafeDictionary;
			void explicitSafeDictionary;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(2);
		expect(diagnostics.filter((diagnostic) => diagnostic.message.includes("unknown"))).toHaveLength(
			1,
		);
		expect(diagnostics.filter((diagnostic) => diagnostic.message.includes("any"))).toHaveLength(1);
	});

	it("resolves interface heritage through aliases and transparent wrappers", () => {
		const diagnostics = lintAntiSlopFixture(
			`type Base<Value> = { [key: string]: Value };
			interface AliasDictionary<Value> extends Base<Value> {}
			interface ReadonlyDictionary<Value> extends Readonly<Base<Value>> {}
			const aliasUnsafe: AliasDictionary<unknown> = {};
			const aliasSafe: AliasDictionary<string> = {};
			const readonlyUnsafe: ReadonlyDictionary<any> = {};
			const readonlySafe: ReadonlyDictionary<string> = {};
			void aliasUnsafe;
			void aliasSafe;
			void readonlyUnsafe;
			void readonlySafe;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(2);
		expect(diagnostics.filter((diagnostic) => diagnostic.message.includes("unknown"))).toHaveLength(
			1,
		);
		expect(diagnostics.filter((diagnostic) => diagnostic.message.includes("any"))).toHaveLength(1);
	});

	it("resolves wrapper-named type parameters before built-in wrappers", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface ReadonlyDictionary<Readonly> {
				[key: string]: Readonly;
			}
			interface PartialDictionary<Partial> {
				[key: string]: Partial;
			}
			interface RequiredDictionary<Required> {
				[key: string]: Required;
			}
			interface NonNullableDictionary<NonNullable> {
				[key: string]: NonNullable;
			}
			interface ValueDictionary<Value> {
				[key: string]: Value;
			}
			const readonlyParameter: ReadonlyDictionary<unknown> = {};
			const partialParameter: PartialDictionary<unknown> = {};
			const requiredParameter: RequiredDictionary<unknown> = {};
			const nonNullableParameter: NonNullableDictionary<unknown> = {};
			const builtInUnsafe: ValueDictionary<Readonly<unknown>> = {};
			const builtInSafe: ValueDictionary<Readonly<string>> = {};
			void readonlyParameter;
			void partialParameter;
			void requiredParameter;
			void nonNullableParameter;
			void builtInUnsafe;
			void builtInSafe;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(5);
		expect(diagnostics.every((diagnostic) => diagnostic.message.includes("unknown"))).toBe(true);
	});

	it("terminates mixed alias and interface heritage cycles", () => {
		const diagnostics = lintAntiSlopFixture(
			`type AliasCycle<Value> = InterfaceCycle<Value>;
			type Base<Value> = { [key: string]: Value };
			interface InterfaceCycle<Value> extends AliasCycle<Value>, Base<Value> {}
			const unsafeCycle: InterfaceCycle<unknown> = {};
			const safeCycle: InterfaceCycle<string> = {};
			void unsafeCycle;
			void safeCycle;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(1);
	});

	it("reports one concrete interface definition once for two consumers", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface DirectUnsafeDictionary {
				[key: string]: unknown;
			}
			const firstDirectConsumer: DirectUnsafeDictionary = {};
			const secondDirectConsumer: DirectUnsafeDictionary = {};
			void firstDirectConsumer;
			void secondDirectConsumer;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(diagnostics[0]?.message).toContain("unknown");
	});

	it("keeps generic, defaulted, and inherited-substitution consumers reportable", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface GenericDictionary<Value> {
				[key: string]: Value;
			}
			interface InheritedUnsafeDictionary extends GenericDictionary<unknown> {}
			interface DefaultDictionary<Value> {
				[key: string]: Value;
			}
			interface DefaultDictionary<Value = unknown> {}
			const genericConsumer: GenericDictionary<unknown> = {};
			const inheritedConsumer: InheritedUnsafeDictionary = {};
			const defaultConsumer: DefaultDictionary = {};
			void genericConsumer;
			void inheritedConsumer;
			void defaultConsumer;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(3);
		expect(diagnostics.every((diagnostic) => diagnostic.message.includes("unknown"))).toBe(true);
	});

	it("keeps safe, non-dictionary, and cyclic interfaces opaque", () => {
		const safeDiagnostics = lintAntiSlopFixture(
			`interface SafeDictionary {
				[key: string]: string;
			}
			const safeDictionary: SafeDictionary = {};
			void safeDictionary;`,
			["no-unsafe-dictionary-type"],
		);
		const opaqueDiagnostics = lintAntiSlopFixture(
			`interface GenericObject<Value> {
				value: Value;
			}
			interface SelfCycle<Value> extends SelfCycle<Value> {}
			interface LeftCycle<Value> extends RightCycle<Value> {}
			interface RightCycle<Value> extends LeftCycle<Value> {}
			const objectValue: GenericObject<unknown> = { value: "known" };
			const selfCycle: SelfCycle<unknown> = { known: "value" };
			const mutualCycle = { known: "value" } as LeftCycle<unknown>;
			void objectValue;
			void selfCycle;
			void mutualCycle;`,
			["no-unsafe-dictionary-type", "no-known-value-widening"],
		);

		expect(safeDiagnostics).toEqual([]);
		expect(opaqueDiagnostics).toEqual([]);
	});

	it("preserves alias, mapped, and empty-interface dictionary handling", () => {
		const diagnostics = lintAntiSlopFixture(
			`type AliasDictionary<Value> = { [key: string]: Value };
			type MappedDictionary<Key extends string, Value> = { [Property in Key]: Value };
			interface EmptyValue {}
			type EmptyValueDictionary = { [key: string]: EmptyValue };
			const aliasDictionary: AliasDictionary<unknown> = { known: "value" };
			const mappedDictionary: MappedDictionary<string, unknown> = { known: "value" };
			const emptyValueDictionary: EmptyValueDictionary = { known: {} };
			void aliasDictionary;
			void mappedDictionary;
			void emptyValueDictionary;`,
			["no-unsafe-dictionary-type", "no-known-value-widening"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(3);
		expect(diagnosticCount(diagnostics, "no-known-value-widening")).toBe(3);
		expect(
			diagnostics.filter((diagnostic) => diagnostic.message.includes("generic container")),
		).toHaveLength(2);
		expect(
			diagnostics.filter((diagnostic) => diagnostic.message.includes("open dictionary")),
		).toHaveLength(1);
	});

	it("preserves built-in and shadowed Record handling", () => {
		const builtInDiagnostics = lintAntiSlopFixture(
			`const dictionary: Record<string, unknown> = { known: "value" };
			void dictionary;`,
			["no-unsafe-dictionary-type", "no-known-value-widening"],
		);
		const shadowedDiagnostics = lintAntiSlopFixture(
			`interface Record<Key, Value> {
				key: Key;
				value: Value;
			}
			const record: Record<string, unknown> = { key: "known", value: "known" };
			void record;`,
			["no-unsafe-dictionary-type", "no-known-value-widening"],
		);

		expect(diagnosticCount(builtInDiagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(diagnosticCount(builtInDiagnostics, "no-known-value-widening")).toBe(1);
		expect(
			builtInDiagnostics.some((diagnostic) => diagnostic.message.includes("open dictionary")),
		).toBe(true);
		expect(shadowedDiagnostics).toEqual([]);
	});

	it("drops a non-generic inherited unsafe index signature narrowed by a same-key-kind override", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base {
				[key: string]: unknown;
			}
			interface Derived extends Base {
				[key: string]: string;
			}
			const derived: Derived = {};
			void derived;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(diagnostics[0]?.message).toContain("unknown");
	});

	it("drops a generic inherited unsafe index signature narrowed by a same-key-kind override", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base<Value> {
				[key: string]: Value;
			}
			interface Derived extends Base<unknown> {
				[key: string]: string;
			}
			const derived: Derived = {};
			void derived;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnostics).toEqual([]);
	});

	it("keeps reporting a derived consumer when its body declares no override of an inherited unsafe signature", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base {
				[key: string]: unknown;
			}
			interface Derived extends Base {}
			const derived: Derived = {};
			void derived;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(2);
		expect(diagnostics.every((diagnostic) => diagnostic.message.includes("unknown"))).toBe(true);
	});

	it("still reports a partial override that leaves an inherited unsafe key kind in place", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base {
				[key: string]: unknown;
				[key: number]: unknown;
			}
			interface Derived extends Base {
				[key: string]: string;
			}
			const derived: Derived = {};
			void derived;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(3);
		expect(diagnostics.every((diagnostic) => diagnostic.message.includes("unknown"))).toBe(true);
	});

	it("keeps the inherited unsafe signature when the direct override covers a different key kind", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base {
				[key: string]: unknown;
			}
			interface Derived extends Base {
				[key: number]: string;
			}
			const derived: Derived = {};
			void derived;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(2);
		expect(diagnostics.every((diagnostic) => diagnostic.message.includes("unknown"))).toBe(true);
	});

	it("drops an inherited unsafe union value narrowed to a safe member by an override", () => {
		const diagnostics = lintAntiSlopFixture(
			`interface Base {
				[key: string]: unknown | string;
			}
			interface Derived extends Base {
				[key: string]: string;
			}
			const derived: Derived = {};
			void derived;`,
			["no-unsafe-dictionary-type"],
		);

		expect(diagnosticCount(diagnostics, "no-unsafe-dictionary-type")).toBe(1);
		expect(diagnostics[0]?.message).toContain("union");
	});
});
