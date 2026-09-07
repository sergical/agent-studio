import type { ESTree } from "@oxlint/plugins";

const BUILT_INS = new Set([
	"Record",
	"Readonly",
	"Partial",
	"Required",
	"Pick",
	"Omit",
	"PropertyKey",
	"NonNullable",
]);
const TRANSPARENT_WRAPPERS = new Set(["Readonly", "Partial", "Required", "NonNullable"]);

type ResolvedType = {
	readonly type: ESTree.TSType;
	readonly substitutions: TypeSubstitutionEnvironment;
};

const UNRESOLVED_TYPE_PARAMETER = Symbol("unresolved type parameter");

type TypeSubstitution = ResolvedType | typeof UNRESOLVED_TYPE_PARAMETER;

type TypeSubstitutionEnvironment = ReadonlyMap<string, TypeSubstitution>;

export type UnsafeDictionary = {
	readonly kind: "unsafe-dictionary";
	readonly unsafeValue: "any" | "empty-object" | "object" | "union" | "unknown";
};

export type WideningTargetKind =
	| "anonymous object"
	| "generic container"
	| "object"
	| "open dictionary"
	| "unknown";

export type WideningTarget = {
	readonly kind: WideningTargetKind;
};

export type TypeEnvironment = {
	readonly aliases: ReadonlyMap<string, ESTree.TSTypeAliasDeclaration>;
	readonly interfaces: ReadonlyMap<string, readonly ESTree.TSInterfaceDeclaration[]>;
	readonly shadowedBuiltIns: ReadonlySet<string>;
};

function declaredStatement(statement: ESTree.Statement): ESTree.Node | null {
	return statement.type === "ExportNamedDeclaration" ||
		statement.type === "ExportDefaultDeclaration"
		? (statement.declaration ?? null)
		: statement;
}

export function createTypeEnvironment(program: ESTree.Program): TypeEnvironment {
	const aliases = new Map<string, ESTree.TSTypeAliasDeclaration>();
	const interfaces = new Map<string, ESTree.TSInterfaceDeclaration[]>();
	const shadowedBuiltIns = new Set<string>();

	for (const statement of program.body) {
		const declaration = declaredStatement(statement);
		if (declaration?.type === "ImportDeclaration") {
			for (const specifier of declaration.specifiers) {
				if (BUILT_INS.has(specifier.local.name)) shadowedBuiltIns.add(specifier.local.name);
			}
			continue;
		}

		if (declaration?.type === "TSTypeAliasDeclaration") {
			const existing = aliases.get(declaration.id.name);
			if (existing === undefined) aliases.set(declaration.id.name, declaration);
			else shadowedBuiltIns.add(declaration.id.name);
			if (BUILT_INS.has(declaration.id.name)) shadowedBuiltIns.add(declaration.id.name);
			continue;
		}

		if (declaration?.type === "TSInterfaceDeclaration") {
			const declarations = interfaces.get(declaration.id.name) ?? [];
			declarations.push(declaration);
			interfaces.set(declaration.id.name, declarations);
			if (BUILT_INS.has(declaration.id.name)) shadowedBuiltIns.add(declaration.id.name);
			continue;
		}

		if (declaration?.type === "TSEnumDeclaration") {
			if (BUILT_INS.has(declaration.id.name)) shadowedBuiltIns.add(declaration.id.name);
			continue;
		}

		if (
			(declaration?.type === "ClassDeclaration" ||
				declaration?.type === "FunctionDeclaration") &&
			declaration.id !== null
		) {
			if (BUILT_INS.has(declaration.id.name)) shadowedBuiltIns.add(declaration.id.name);
		}
	}

	return { aliases, interfaces, shadowedBuiltIns };
}

function typeReferenceName(type: ESTree.TSTypeReference): string | null {
	return type.typeName.type === "Identifier" ? type.typeName.name : null;
}

function isBuiltIn(name: string, environment: TypeEnvironment): boolean {
	return BUILT_INS.has(name) && !environment.shadowedBuiltIns.has(name);
}

function unwrapTransparentType(type: ESTree.TSType): ESTree.TSType {
	let current = type;
	while (
		current.type === "TSParenthesizedType" ||
		(current.type === "TSTypeOperator" && current.operator === "readonly")
	) {
		current = current.typeAnnotation;
	}
	return current;
}

function isNeverType(type: ESTree.TSType): boolean {
	return unwrapTransparentType(type).type === "TSNeverKeyword";
}

function isEffectivelyEmptyMember(member: ESTree.TSSignature): boolean {
	return (
		member.type === "TSPropertySignature" &&
		member.optional === true &&
		member.typeAnnotation !== null &&
		member.typeAnnotation !== undefined &&
		isNeverType(member.typeAnnotation.typeAnnotation)
	);
}

function isEffectivelyEmptyTypeLiteral(type: ESTree.TSTypeLiteral): boolean {
	return type.members.length === 0 || type.members.every(isEffectivelyEmptyMember);
}

function isEffectivelyEmptyInterface(
	declarations: readonly ESTree.TSInterfaceDeclaration[],
): boolean {
	if (declarations.length !== 1) return false;
	const [type] = declarations;
	return (
		type !== undefined &&
		type.extends.length === 0 &&
		(type.body.body.length === 0 || type.body.body.every(isEffectivelyEmptyMember))
	);
}

function resolvedTypeArgument(
	type: ESTree.TSType,
	substitutions: TypeSubstitutionEnvironment,
): TypeSubstitution {
	const unwrapped = unwrapTransparentType(type);
	const name = unwrapped.type === "TSTypeReference" ? typeReferenceName(unwrapped) : null;
	return (name === null ? undefined : substitutions.get(name)) ?? {
		type,
		substitutions: new Map(substitutions),
	};
}

function typeParameterSubstitutions(
	typeParameters: ESTree.TSTypeParameterDeclaration | null,
	typeArguments: ESTree.TSTypeParameterInstantiation | null | undefined,
	callerSubstitutions: TypeSubstitutionEnvironment,
	defaultArguments: readonly (ESTree.TSType | undefined)[] = [],
): TypeSubstitutionEnvironment | null {
	const parameters = typeParameters?.params ?? [];
	const arguments_ = typeArguments?.params ?? [];
	const resolvedArguments = arguments_.map((argument) =>
		resolvedTypeArgument(argument, callerSubstitutions),
	);
	const next: Map<string, TypeSubstitution> = new Map();
	for (const [index, parameter] of parameters.entries()) {
		const argument = resolvedArguments[index];
		if (argument === undefined) break;
		next.set(parameter.name.name, argument);
	}
	for (const [index, parameter] of parameters.entries()) {
		if (index < resolvedArguments.length) continue;
		const argument = parameter.default ?? defaultArguments[index];
		if (argument === null || argument === undefined) return null;
		next.set(parameter.name.name, resolvedTypeArgument(argument, next));
	}
	return next;
}

function genericScopeTypeParameters(
	node: ESTree.Node,
): ESTree.TSTypeParameterDeclaration | null {
	switch (node.type) {
		case "FunctionDeclaration":
		case "FunctionExpression":
		case "TSDeclareFunction":
		case "TSEmptyBodyFunctionExpression":
		case "ArrowFunctionExpression":
		case "ClassDeclaration":
		case "ClassExpression":
		case "TSTypeAliasDeclaration":
		case "TSInterfaceDeclaration":
		case "TSCallSignatureDeclaration":
		case "TSConstructSignatureDeclaration":
		case "TSMethodSignature":
		case "TSFunctionType":
		case "TSConstructorType":
			return node.typeParameters ?? null;
		default:
			return null;
	}
}

function collectInferTypeParameterNamesFromTypeParameters(
	typeParameters: ESTree.TSTypeParameterDeclaration | null,
	names: Set<string>,
): void {
	for (const parameter of typeParameters?.params ?? []) {
		if (parameter.constraint !== null) {
			collectInferTypeParameterNames(parameter.constraint, names);
		}
		if (parameter.default !== null) collectInferTypeParameterNames(parameter.default, names);
	}
}

function collectInferTypeParameterNamesFromTypeArguments(
	typeArguments: ESTree.TSTypeParameterInstantiation | null,
	names: Set<string>,
): void {
	for (const argument of typeArguments?.params ?? []) {
		collectInferTypeParameterNames(argument, names);
	}
}

function collectInferTypeParameterNamesFromTypeAnnotation(
	typeAnnotation: ESTree.TSTypeAnnotation | null | undefined,
	names: Set<string>,
): void {
	if (typeAnnotation !== null && typeAnnotation !== undefined) {
		collectInferTypeParameterNames(typeAnnotation.typeAnnotation, names);
	}
}

function collectInferTypeParameterNamesFromBindingPattern(
	pattern: ESTree.BindingPattern,
	names: Set<string>,
): void {
	switch (pattern.type) {
		case "Identifier":
			collectInferTypeParameterNamesFromTypeAnnotation(pattern.typeAnnotation, names);
			return;
		case "AssignmentPattern":
			collectInferTypeParameterNamesFromBindingPattern(pattern.left, names);
			return;
		case "ObjectPattern":
			collectInferTypeParameterNamesFromTypeAnnotation(pattern.typeAnnotation, names);
			for (const property of pattern.properties) {
				if (property.type === "Property") {
					collectInferTypeParameterNamesFromBindingPattern(property.value, names);
				} else {
					collectInferTypeParameterNamesFromTypeAnnotation(property.typeAnnotation, names);
					collectInferTypeParameterNamesFromBindingPattern(property.argument, names);
				}
			}
			return;
		case "ArrayPattern":
			collectInferTypeParameterNamesFromTypeAnnotation(pattern.typeAnnotation, names);
			for (const element of pattern.elements) {
				if (element === null) continue;
				if (element.type === "RestElement") {
					collectInferTypeParameterNamesFromTypeAnnotation(element.typeAnnotation, names);
					collectInferTypeParameterNamesFromBindingPattern(element.argument, names);
				} else {
					collectInferTypeParameterNamesFromBindingPattern(element, names);
				}
			}
	}
}

function collectInferTypeParameterNamesFromParameters(
	parameters: readonly ESTree.ParamPattern[],
	names: Set<string>,
): void {
	for (const parameter of parameters) {
		if (parameter.type === "TSParameterProperty") {
			collectInferTypeParameterNamesFromBindingPattern(parameter.parameter, names);
		} else if (parameter.type === "RestElement") {
			collectInferTypeParameterNamesFromTypeAnnotation(parameter.typeAnnotation, names);
			collectInferTypeParameterNamesFromBindingPattern(parameter.argument, names);
		} else {
			collectInferTypeParameterNamesFromBindingPattern(parameter, names);
		}
	}
}

function collectInferTypeParameterNamesFromSignature(
	signature: ESTree.TSSignature,
	names: Set<string>,
): void {
	switch (signature.type) {
		case "TSPropertySignature":
			collectInferTypeParameterNamesFromTypeAnnotation(signature.typeAnnotation, names);
			return;
		case "TSIndexSignature":
			for (const parameter of signature.parameters) {
				collectInferTypeParameterNamesFromTypeAnnotation(parameter.typeAnnotation, names);
			}
			collectInferTypeParameterNamesFromTypeAnnotation(signature.typeAnnotation, names);
			return;
		case "TSCallSignatureDeclaration":
		case "TSConstructSignatureDeclaration":
		case "TSMethodSignature":
			collectInferTypeParameterNamesFromTypeParameters(signature.typeParameters, names);
			collectInferTypeParameterNamesFromParameters(signature.params, names);
			collectInferTypeParameterNamesFromTypeAnnotation(signature.returnType, names);
	}
}

function collectInferTypeParameterNamesFromTupleElement(
	element: ESTree.TSTupleElement,
	names: Set<string>,
): void {
	if (element.type === "TSOptionalType" || element.type === "TSRestType") {
		collectInferTypeParameterNames(element.typeAnnotation, names);
	} else {
		collectInferTypeParameterNames(element, names);
	}
}

function collectInferTypeParameterNames(type: ESTree.TSType, names: Set<string>): void {
	switch (type.type) {
		case "TSArrayType":
			collectInferTypeParameterNames(type.elementType, names);
			return;
		case "TSConditionalType":
			collectInferTypeParameterNames(type.checkType, names);
			collectInferTypeParameterNames(type.extendsType, names);
			collectInferTypeParameterNames(type.trueType, names);
			collectInferTypeParameterNames(type.falseType, names);
			return;
		case "TSConstructorType":
		case "TSFunctionType":
			collectInferTypeParameterNamesFromTypeParameters(type.typeParameters, names);
			collectInferTypeParameterNamesFromParameters(type.params, names);
			collectInferTypeParameterNamesFromTypeAnnotation(type.returnType, names);
			return;
		case "TSImportType":
			collectInferTypeParameterNamesFromTypeArguments(type.typeArguments, names);
			return;
		case "TSIndexedAccessType":
			collectInferTypeParameterNames(type.objectType, names);
			collectInferTypeParameterNames(type.indexType, names);
			return;
		case "TSInferType":
			names.add(type.typeParameter.name.name);
			if (type.typeParameter.constraint !== null) {
				collectInferTypeParameterNames(type.typeParameter.constraint, names);
			}
			if (type.typeParameter.default !== null) {
				collectInferTypeParameterNames(type.typeParameter.default, names);
			}
			return;
		case "TSIntersectionType":
		case "TSUnionType":
			for (const member of type.types) collectInferTypeParameterNames(member, names);
			return;
		case "TSMappedType":
			collectInferTypeParameterNames(type.constraint, names);
			if (type.nameType !== null) collectInferTypeParameterNames(type.nameType, names);
			if (type.typeAnnotation !== null) collectInferTypeParameterNames(type.typeAnnotation, names);
			return;
		case "TSNamedTupleMember":
			collectInferTypeParameterNamesFromTupleElement(type.elementType, names);
			return;
		case "TSParenthesizedType":
		case "TSTypeOperator":
		case "TSJSDocNonNullableType":
		case "TSJSDocNullableType":
			collectInferTypeParameterNames(type.typeAnnotation, names);
			return;
		case "TSTemplateLiteralType":
			for (const embeddedType of type.types) {
				collectInferTypeParameterNames(embeddedType, names);
			}
			return;
		case "TSTupleType":
			for (const element of type.elementTypes) {
				collectInferTypeParameterNamesFromTupleElement(element, names);
			}
			return;
		case "TSTypeLiteral":
			for (const signature of type.members) {
				collectInferTypeParameterNamesFromSignature(signature, names);
			}
			return;
		case "TSTypePredicate":
			collectInferTypeParameterNamesFromTypeAnnotation(type.typeAnnotation, names);
			return;
		case "TSTypeQuery":
			if (type.exprName.type === "TSImportType") {
				collectInferTypeParameterNames(type.exprName, names);
			}
			collectInferTypeParameterNamesFromTypeArguments(type.typeArguments, names);
			return;
		case "TSTypeReference":
			collectInferTypeParameterNamesFromTypeArguments(type.typeArguments, names);
			return;
		case "TSAnyKeyword":
		case "TSBigIntKeyword":
		case "TSBooleanKeyword":
		case "TSIntrinsicKeyword":
		case "TSLiteralType":
		case "TSNeverKeyword":
		case "TSNullKeyword":
		case "TSNumberKeyword":
		case "TSObjectKeyword":
		case "TSStringKeyword":
		case "TSSymbolKeyword":
		case "TSThisType":
		case "TSUndefinedKeyword":
		case "TSUnknownKeyword":
		case "TSVoidKeyword":
		case "TSJSDocUnknownType":
			return;
	}
}

function bindUnresolvedTypeParameter(
	name: string,
	substitutions: Map<string, TypeSubstitution>,
): void {
	if (!substitutions.has(name)) substitutions.set(name, UNRESOLVED_TYPE_PARAMETER);
}

function lexicalTypeParameterSubstitutions(type: ESTree.TSType): TypeSubstitutionEnvironment {
	const substitutions = new Map<string, TypeSubstitution>();
	let descendant: ESTree.Node = type;
	let current: ESTree.Node | null = type.parent;
	while (current !== null) {
		for (const parameter of genericScopeTypeParameters(current)?.params ?? []) {
			bindUnresolvedTypeParameter(parameter.name.name, substitutions);
		}
		if (
			current.type === "TSMappedType" &&
			(current.typeAnnotation === descendant || current.nameType === descendant)
		) {
			bindUnresolvedTypeParameter(current.key.name, substitutions);
		}
		if (current.type === "TSConditionalType" && current.trueType === descendant) {
			const inferTypeParameterNames = new Set<string>();
			collectInferTypeParameterNames(current.extendsType, inferTypeParameterNames);
			for (const name of inferTypeParameterNames) {
				bindUnresolvedTypeParameter(name, substitutions);
			}
		}
		descendant = current;
		current = current.parent;
	}
	return substitutions;
}

function mergedInterfaceTypeParameterDefaults(
	declarations: readonly ESTree.TSInterfaceDeclaration[],
): readonly (ESTree.TSType | undefined)[] {
	const defaults: (ESTree.TSType | undefined)[] = [];
	for (const declaration of declarations) {
		for (const [index, parameter] of (declaration.typeParameters?.params ?? []).entries()) {
			if (defaults[index] === undefined && parameter.default !== null) {
				defaults[index] = parameter.default;
			}
		}
	}
	return defaults;
}

function unsafeDirectValue(
	type: ESTree.TSType,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
	resolvingAliases: ReadonlySet<string>,
): UnsafeDictionary["unsafeValue"] | null {
	const unwrapped = unwrapTransparentType(type);
	if (unwrapped.type === "TSUnknownKeyword") return "unknown";
	if (unwrapped.type === "TSAnyKeyword") return "any";
	if (unwrapped.type === "TSObjectKeyword") return "object";
	if (unwrapped.type === "TSTypeLiteral" && isEffectivelyEmptyTypeLiteral(unwrapped))
		return "empty-object";
	if (unwrapped.type === "TSUnionType") {
		return unwrapped.types.some(
			(member) => unsafeDirectValue(member, environment, substitutions, resolvingAliases) !== null,
		)
			? "union"
			: null;
	}
	if (unwrapped.type === "TSIntersectionType") {
		const unsafeMembers = unwrapped.types.map((member) =>
			unsafeDirectValue(member, environment, substitutions, resolvingAliases),
		);
		if (unsafeMembers.includes("any")) return "any";
		return unsafeMembers.length > 0 && unsafeMembers.every((member) => member !== null)
			? unsafeMembers[0]
			: null;
	}
	if (unwrapped.type !== "TSTypeReference") return null;
	const name = typeReferenceName(unwrapped);
	if (name === null) return null;
	const substitution = substitutions.get(name);
	if (substitution !== undefined) {
		if (substitution === UNRESOLVED_TYPE_PARAMETER) return null;
		return unsafeDirectValue(
			substitution.type,
			environment,
			substitution.substitutions,
			resolvingAliases,
		);
	}
	if (TRANSPARENT_WRAPPERS.has(name) && isBuiltIn(name, environment)) {
		const wrapped = unwrapped.typeArguments?.params[0];
		return wrapped === undefined
			? null
			: unsafeDirectValue(wrapped, environment, substitutions, resolvingAliases);
	}
	const interfaceDeclarations = environment.interfaces.get(name);
	if (interfaceDeclarations !== undefined) {
		return isEffectivelyEmptyInterface(interfaceDeclarations) ? "empty-object" : null;
	}
	const alias = environment.aliases.get(name);
	if (alias === undefined || resolvingAliases.has(name)) return null;
	const nextSubstitutions = typeParameterSubstitutions(
		alias.typeParameters,
		unwrapped.typeArguments,
		substitutions,
	);
	if (nextSubstitutions === null) return null;
	const nextResolving = new Set(resolvingAliases);
	nextResolving.add(name);
	return unsafeDirectValue(alias.typeAnnotation, environment, nextSubstitutions, nextResolving);
}

function dictionaryValueTypes(
	type: ESTree.TSType,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
	resolvingAliases: ReadonlySet<string>,
	resolvingInterfaces: ReadonlySet<string>,
): readonly ResolvedType[] {
	const unwrapped = unwrapTransparentType(type);

	if (unwrapped.type === "TSTypeLiteral") {
		return unwrapped.members.flatMap((member): readonly ResolvedType[] =>
			member.type === "TSIndexSignature" && member.typeAnnotation !== null
				? [{ type: member.typeAnnotation.typeAnnotation, substitutions }]
				: [],
		);
	}

	if (unwrapped.type === "TSMappedType") {
		const valueSubstitutions = new Map(substitutions);
		valueSubstitutions.set(unwrapped.key.name, UNRESOLVED_TYPE_PARAMETER);
		return unwrapped.typeAnnotation === null
			? []
			: [{ type: unwrapped.typeAnnotation, substitutions: valueSubstitutions }];
	}

	if (unwrapped.type !== "TSTypeReference") return [];
	const name = typeReferenceName(unwrapped);
	if (name === null) return [];
	return namedDictionaryValueTypes(
		name,
		unwrapped.typeArguments,
		environment,
		substitutions,
		resolvingAliases,
		resolvingInterfaces,
	);
}

function namedDictionaryValueTypes(
	name: string,
	typeArguments: ESTree.TSTypeParameterInstantiation | null | undefined,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
	resolvingAliases: ReadonlySet<string>,
	resolvingInterfaces: ReadonlySet<string>,
): readonly ResolvedType[] {
	const substitution = substitutions.get(name);
	if (substitution !== undefined) {
		if (substitution === UNRESOLVED_TYPE_PARAMETER) return [];
		return dictionaryValueTypes(
			substitution.type,
			environment,
			substitution.substitutions,
			resolvingAliases,
			resolvingInterfaces,
		);
	}

	if (TRANSPARENT_WRAPPERS.has(name) && isBuiltIn(name, environment)) {
		const wrapped = typeArguments?.params[0];
		return wrapped === undefined
			? []
			: dictionaryValueTypes(
					wrapped,
					environment,
					substitutions,
					resolvingAliases,
					resolvingInterfaces,
				);
	}

	if (name === "Record" && isBuiltIn(name, environment)) {
		const value = typeArguments?.params[1] ?? null;
		return value === null ? [] : [{ type: value, substitutions }];
	}

	if ((name === "Pick" || name === "Omit") && isBuiltIn(name, environment)) {
		const source = typeArguments?.params[0];
		return source === undefined
			? []
			: dictionaryValueTypes(
					source,
					environment,
					substitutions,
					resolvingAliases,
					resolvingInterfaces,
				);
	}

	const interfaceDeclarations = environment.interfaces.get(name);
	if (interfaceDeclarations !== undefined) {
		return interfaceDictionaryValueTypes(
			name,
			interfaceDeclarations,
			typeArguments,
			environment,
			substitutions,
			resolvingAliases,
			resolvingInterfaces,
		);
	}

	const alias = environment.aliases.get(name);
	if (alias === undefined || resolvingAliases.has(name)) return [];
	const nextSubstitutions = typeParameterSubstitutions(
		alias.typeParameters,
		typeArguments,
		substitutions,
	);
	if (nextSubstitutions === null) return [];
	const nextResolving = new Set(resolvingAliases);
	nextResolving.add(name);
	return dictionaryValueTypes(
		alias.typeAnnotation,
		environment,
		nextSubstitutions,
		nextResolving,
		resolvingInterfaces,
	);
}

function interfaceDictionaryValueTypes(
	name: string,
	declarations: readonly ESTree.TSInterfaceDeclaration[],
	typeArguments: ESTree.TSTypeParameterInstantiation | null | undefined,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
	resolvingAliases: ReadonlySet<string>,
	resolvingInterfaces: ReadonlySet<string>,
): readonly ResolvedType[] {
	if (resolvingInterfaces.has(name)) return [];
	const nextResolvingInterfaces = new Set(resolvingInterfaces);
	nextResolvingInterfaces.add(name);
	const defaultArguments = mergedInterfaceTypeParameterDefaults(declarations);

	return declarations.flatMap((declaration): readonly ResolvedType[] => {
		const nextSubstitutions = typeParameterSubstitutions(
			declaration.typeParameters,
			typeArguments,
			substitutions,
			defaultArguments,
		);
		if (nextSubstitutions === null) return [];
		const directValueTypes = declaration.body.body.flatMap((member): readonly ResolvedType[] =>
			member.type === "TSIndexSignature"
				? [{ type: member.typeAnnotation.typeAnnotation, substitutions: nextSubstitutions }]
				: [],
		);
		const inheritedValueTypes = declaration.extends.flatMap((heritage): readonly ResolvedType[] => {
			if (heritage.expression.type !== "Identifier") return [];
			return namedDictionaryValueTypes(
				heritage.expression.name,
				heritage.typeArguments,
				environment,
				nextSubstitutions,
				resolvingAliases,
				nextResolvingInterfaces,
			);
		});
		return [...directValueTypes, ...inheritedValueTypes];
	});
}

export function classifyUnsafeDictionaryValue(
	valueType: ESTree.TSType,
	environment: TypeEnvironment,
): UnsafeDictionary | null {
	const unsafeValue = unsafeDirectValue(
		valueType,
		environment,
		lexicalTypeParameterSubstitutions(valueType),
		new Set(),
	);
	return unsafeValue === null ? null : { kind: "unsafe-dictionary", unsafeValue };
}

export function classifyUnsafeDictionary(
	type: ESTree.TSType,
	environment: TypeEnvironment,
): UnsafeDictionary | null {
	for (const valueType of dictionaryValueTypes(
		type,
		environment,
		lexicalTypeParameterSubstitutions(type),
		new Set(),
		new Set(),
	)) {
		const unsafeValue = unsafeDirectValue(
			valueType.type,
			environment,
			valueType.substitutions,
			new Set(),
		);
		if (unsafeValue !== null) return { kind: "unsafe-dictionary", unsafeValue };
	}
	return null;
}

function resolvesToDictionary(
	type: ESTree.TSType,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
	resolvingAliases: ReadonlySet<string>,
	resolvingInterfaces: ReadonlySet<string>,
): boolean {
	return (
		dictionaryValueTypes(type, environment, substitutions, resolvingAliases, resolvingInterfaces)
			.length > 0
	);
}

export function classifyWideningTarget(
	type: ESTree.TSType,
	environment: TypeEnvironment,
): WideningTarget | null {
	return classifyWideningTargetInScope(
		type,
		environment,
		lexicalTypeParameterSubstitutions(type),
	);
}

function classifyWideningTargetInScope(
	type: ESTree.TSType,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
): WideningTarget | null {
	const unwrapped = unwrapTransparentType(type);
	if (unwrapped.type === "TSUnknownKeyword") return { kind: "unknown" };
	if (unwrapped.type === "TSObjectKeyword") return { kind: "object" };
	if (unwrapped.type === "TSTypeLiteral") {
		return unwrapped.members.some((member) => member.type === "TSIndexSignature")
			? { kind: "open dictionary" }
			: unwrapped.members.length > 0
				? { kind: "anonymous object" }
				: null;
	}
	if (unwrapped.type === "TSMappedType") return { kind: "open dictionary" };
	if (unwrapped.type !== "TSTypeReference") return null;
	const name = typeReferenceName(unwrapped);
	if (name === null) return null;
	const substitution = substitutions.get(name);
	if (substitution !== undefined) {
		return substitution === UNRESOLVED_TYPE_PARAMETER
			? null
			: classifyWideningTargetInScope(
					substitution.type,
					environment,
					substitution.substitutions,
				);
	}
	if (TRANSPARENT_WRAPPERS.has(name) && isBuiltIn(name, environment)) {
		const wrapped = unwrapped.typeArguments?.params[0];
		return wrapped === undefined
			? null
			: classifyWideningTargetInScope(wrapped, environment, substitutions);
	}
	if (name === "Record" && isBuiltIn(name, environment)) return { kind: "open dictionary" };
	const interfaceDeclarations = environment.interfaces.get(name);
	if (interfaceDeclarations !== undefined) {
		return resolvesToDictionary(unwrapped, environment, substitutions, new Set(), new Set())
			? {
					kind: interfaceDeclarations.some(
						(declaration) => (declaration.typeParameters?.params.length ?? 0) > 0,
					)
						? "generic container"
						: "open dictionary",
				}
			: null;
	}
	const alias = environment.aliases.get(name);
	if (alias === undefined) return null;
	const calleeSubstitutions = typeParameterSubstitutions(
		alias.typeParameters,
		unwrapped.typeArguments,
		substitutions,
	);
	if (calleeSubstitutions === null) return null;
	if ((alias.typeParameters?.params.length ?? 0) > 0) {
		return resolvesToDictionary(
			alias.typeAnnotation,
			environment,
			calleeSubstitutions,
			new Set([name]),
			new Set(),
		)
			? { kind: "generic container" }
			: null;
	}
	return classifyAliasBroadTarget(
		alias.typeAnnotation,
		environment,
		calleeSubstitutions,
		new Set([name]),
	);
}

function isBroadMappedKey(
	type: ESTree.TSType,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
): boolean {
	const unwrapped = unwrapTransparentType(type);
	if (
		unwrapped.type === "TSStringKeyword" ||
		unwrapped.type === "TSNumberKeyword" ||
		unwrapped.type === "TSSymbolKeyword"
	) {
		return true;
	}
	if (unwrapped.type === "TSUnionType") {
		return unwrapped.types.every((member) =>
			isBroadMappedKey(member, environment, substitutions),
		);
	}
	if (unwrapped.type !== "TSTypeReference") return false;
	const name = typeReferenceName(unwrapped);
	if (name === null) return false;
	const substitution = substitutions.get(name);
	if (substitution !== undefined) {
		if (substitution === UNRESOLVED_TYPE_PARAMETER) return false;
		return isBroadMappedKey(substitution.type, environment, substitution.substitutions);
	}
	return name === "PropertyKey" && isBuiltIn(name, environment);
}

function classifyAliasBroadTarget(
	type: ESTree.TSType,
	environment: TypeEnvironment,
	substitutions: TypeSubstitutionEnvironment,
	resolvingAliases: ReadonlySet<string>,
): WideningTarget | null {
	const unwrapped = unwrapTransparentType(type);
	if (unwrapped.type === "TSUnknownKeyword") return { kind: "unknown" };
	if (unwrapped.type === "TSObjectKeyword") return { kind: "object" };
	if (unwrapped.type === "TSTypeLiteral") {
		return unwrapped.members.some((member) => member.type === "TSIndexSignature")
			? { kind: "open dictionary" }
			: null;
	}
	if (unwrapped.type === "TSMappedType") {
		return isBroadMappedKey(unwrapped.constraint, environment, substitutions)
			? { kind: "open dictionary" }
			: null;
	}
	if (unwrapped.type !== "TSTypeReference") return null;
	const name = typeReferenceName(unwrapped);
	if (name === null) return null;
	const substitution = substitutions.get(name);
	if (substitution !== undefined) {
		if (substitution === UNRESOLVED_TYPE_PARAMETER) return null;
		return classifyAliasBroadTarget(
			substitution.type,
			environment,
			substitution.substitutions,
			resolvingAliases,
		);
	}
	if (TRANSPARENT_WRAPPERS.has(name) && isBuiltIn(name, environment)) {
		const wrapped = unwrapped.typeArguments?.params[0];
		return wrapped === undefined
			? null
			: classifyAliasBroadTarget(wrapped, environment, substitutions, resolvingAliases);
	}
	if (name === "Record" && isBuiltIn(name, environment)) {
		return { kind: "open dictionary" };
	}
	if (environment.interfaces.has(name)) {
		return resolvesToDictionary(unwrapped, environment, substitutions, resolvingAliases, new Set())
			? { kind: "open dictionary" }
			: null;
	}
	const alias = environment.aliases.get(name);
	if (alias === undefined || resolvingAliases.has(name)) return null;
	const nextSubstitutions = typeParameterSubstitutions(
		alias.typeParameters,
		unwrapped.typeArguments,
		substitutions,
	);
	if (nextSubstitutions === null) return null;
	const nextResolving = new Set(resolvingAliases);
	nextResolving.add(name);
	return classifyAliasBroadTarget(
		alias.typeAnnotation,
		environment,
		nextSubstitutions,
		nextResolving,
	);
}

export function isPopulatedObjectExpression(expression: ESTree.Expression): boolean {
	let current = expression;
	while (
		current.type === "ParenthesizedExpression" ||
		current.type === "TSAsExpression" ||
		current.type === "TSTypeAssertion" ||
		current.type === "TSNonNullExpression"
	) {
		current = current.expression;
	}
	return current.type === "ObjectExpression" && current.properties.length > 0;
}

export function isKnownEvidenceExpression(expression: ESTree.Expression): boolean {
	let current = expression;
	while (
		current.type === "ParenthesizedExpression" ||
		current.type === "TSAsExpression" ||
		current.type === "TSTypeAssertion" ||
		current.type === "TSNonNullExpression" ||
		current.type === "TSSatisfiesExpression"
	) {
		current = current.expression;
	}
	if (current.type === "ObjectExpression") return true;
	return (
		current.type === "ArrayExpression" ||
		current.type === "ArrowFunctionExpression" ||
		current.type === "ClassExpression" ||
		current.type === "FunctionExpression" ||
		current.type === "NewExpression" ||
		current.type === "Literal" ||
		current.type === "TemplateLiteral" ||
		current.type === "UnaryExpression"
	);
}
