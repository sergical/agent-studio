import { defineRule } from "@oxlint/plugins";

const PRIMITIVE_BY_TAG = {
  input: "Input",
  textarea: "Textarea",
  select: "Select",
  button: "Button",
} satisfies Record<string, string>;

/** Ban raw form elements in favor of the shared @skill-studio/ui primitives, which own focus/ring/border styling. */
export const noRawFormElementsRule = defineRule({
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow raw <input>/<textarea>/<select>/<button> elements; use the matching primitive from @skill-studio/ui.",
    },
    messages: {
      rawElement: "Use the {{element}} primitive from @skill-studio/ui instead of a raw <{{tag}}>.",
    },
  },
  create(context) {
    return {
      JSXOpeningElement(node) {
        if (node.name.type !== "JSXIdentifier") return;
        const tag = node.name.name;
        const element = PRIMITIVE_BY_TAG[tag];
        if (!element) return;
        context.report({ node, messageId: "rawElement", data: { element, tag } });
      },
    };
  },
});
