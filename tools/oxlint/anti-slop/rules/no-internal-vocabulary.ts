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

function isAddToastCall(node: ESTree.CallExpression): boolean {
  const callee = node.callee;
  if (callee.type === "Identifier") return callee.name === "addToast";
  return (
    callee.type === "MemberExpression" &&
    callee.property.type === "Identifier" &&
    callee.property.name === "addToast"
  );
}

/** Keep the developers' vocabulary (deployment, harness, canonical...) out of text a user reads. */
export const noInternalVocabularyRule = defineRule({
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow internal vocabulary in user-facing text: Error messages, toast titles and messages, and JSX text.",
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
      CallExpression(node) {
        if (!isAddToastCall(node)) return;
        const options = node.arguments[0];
        if (options?.type !== "ObjectExpression") return;
        for (const property of options.properties) {
          if (property.type !== "Property" || property.key.type !== "Identifier") continue;
          if (property.key.name === "title" || property.key.name === "message") {
            check(property.value, staticText(property.value));
          }
        }
      },
      JSXText(node) {
        check(node, node.value);
      },
    };
  },
});
