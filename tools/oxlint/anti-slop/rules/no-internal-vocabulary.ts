import { defineRule } from "@oxlint/plugins";

import type { ESTree } from "@oxlint/plugins";

const BANNED_WORDS =
  /\b(deployments?|lifecycle owners?|owner groups?|materializ\w*|mutable|unambiguous|canonical|argv|harnesses|harness)\b/i;

/** The text of a string literal or the fixed parts of a template literal; `null` for anything else. */
function staticText(node: ESTree.Node | null | undefined): string | null {
  if (!node) return null;
  if (node.type === "Literal") return node.value === null ? null : String(node.value);
  if (node.type === "TemplateLiteral") {
    return node.quasis.map((quasi) => quasi.value.cooked ?? "").join(" ");
  }
  return null;
}

const USER_TEXT_KEYS = new Set([
  "label",
  "title",
  "description",
  "subtitle",
  "message",
  "placeholder",
  "content",
  "tooltip",
  "aria-label",
  "triggerAriaLabel",
]);
const USER_TEXT_KEY_SUFFIX = /(Label|Title|Text|Message|Description|Caption)$/;
const USER_TEXT_FUNCTION_SUFFIX = /(Label|Text|Title|Message|Caption|Copy|Note|Reason|Description|Hint)$/;

function isUserTextKey(name: string): boolean {
  return USER_TEXT_KEYS.has(name) || USER_TEXT_KEY_SUFFIX.test(name);
}

function propertyKeyName(node: ESTree.Node): string | null {
  if (node.type === "Identifier") return node.name;
  if (node.type === "Literal") return staticText(node);
  return null;
}

function jsxAttributeName(node: ESTree.JSXAttribute): string {
  return node.name.type === "JSXIdentifier" ? node.name.name : `${node.name.namespace.name}:${node.name.name.name}`;
}

/** The name a function is declared or assigned under, or `null` when it is anonymous. */
function functionName(node: ESTree.Node): string | null {
  if ((node.type === "FunctionDeclaration" || node.type === "FunctionExpression") && node.id) {
    return node.id.name;
  }
  const parent = node.parent;
  if (parent?.type === "VariableDeclarator" && parent.id.type === "Identifier") return parent.id.name;
  if (parent?.type === "Property" || parent?.type === "MethodDefinition") {
    return propertyKeyName(parent.key);
  }
  return null;
}

/** Whether the text node is shown to a user: a user-text prop or key, a JSX child, or a text-returning function. */
function isUserFacingPosition(node: ESTree.Node): boolean {
  let child: ESTree.Node = node;
  let parent = child.parent;
  while (
    parent &&
    (parent.type === "ConditionalExpression"
      ? parent.test !== child
      : parent.type === "LogicalExpression" || parent.type === "TemplateLiteral")
  ) {
    child = parent;
    parent = child.parent;
  }
  if (!parent) return false;
  switch (parent.type) {
    case "JSXAttribute":
      return isUserTextKey(jsxAttributeName(parent));
    case "JSXExpressionContainer": {
      const owner = parent.parent;
      if (owner?.type === "JSXAttribute") return isUserTextKey(jsxAttributeName(owner));
      return owner?.type === "JSXElement" || owner?.type === "JSXFragment";
    }
    case "Property": {
      if (parent.value !== child) return false;
      const key = propertyKeyName(parent.key);
      return key !== null && isUserTextKey(key);
    }
    case "ReturnStatement":
    case "ArrowFunctionExpression": {
      let fn: ESTree.Node | null | undefined = parent;
      while (fn && fn.type !== "FunctionDeclaration" && fn.type !== "FunctionExpression" && fn.type !== "ArrowFunctionExpression") {
        fn = fn.parent;
      }
      if (parent.type === "ArrowFunctionExpression" && parent.body !== child) return false;
      const name = fn ? functionName(fn) : null;
      return name !== null && USER_TEXT_FUNCTION_SUFFIX.test(name);
    }
    default:
      return false;
  }
}

/** Keep the developers' vocabulary (deployment, harness, canonical...) out of text a user reads. */
export const noInternalVocabularyRule = defineRule({
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow internal vocabulary in user-facing text: Error messages, JSX text, user-text props and object keys, JSX children, and text-returning functions.",
    },
    messages: {
      internalWord:
        'User-facing text says "{{word}}". Use plain words: skills, folders, copies, links, agents, projects.',
    },
  },
  create(context) {
    const check = (node: ESTree.Node, text: string | null) => {
      const word = text ? BANNED_WORDS.exec(text)?.[0] : undefined;
      if (word) context.report({ node, messageId: "internalWord", data: { word } });
    };
    return {
      NewExpression(node) {
        if (node.callee.type !== "Identifier" || node.callee.name !== "Error") return;
        const message = node.arguments[0];
        check(node, message?.type === "SpreadElement" ? null : staticText(message));
      },
      Literal(node) {
        if (isUserFacingPosition(node)) check(node, staticText(node));
      },
      TemplateLiteral(node) {
        if (isUserFacingPosition(node)) check(node, staticText(node));
      },
      JSXText(node) {
        check(node, node.value);
      },
    };
  },
});
