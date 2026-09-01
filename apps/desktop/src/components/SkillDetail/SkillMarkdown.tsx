// ============================================================================
// SkillMarkdown - the one SKILL.md/markdown body renderer: react-markdown +
// remark-gfm with Tailwind-styled elements. Prose blocks keep a 72ch reading
// measure; tables and code blocks break out to the full card width with
// their own horizontal scrollers, so a wide table isn't squeezed into the
// prose column.
// ============================================================================

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

interface SkillMarkdownProps {
  /** Markdown source, frontmatter already stripped by the caller. */
  content: string;
  /** Extra classes on the container, e.g. padding when rendered inside a card. */
  className?: string;
}

// Reading-measure cap for running text only - block elements (tables, pre)
// deliberately omit it and take the full container width.
const PROSE = "max-w-[72ch]";

export function SkillMarkdown({ content, className }: SkillMarkdownProps) {
  return (
    <div className={`text-body leading-[1.6] text-text-secondary ${className ?? ""}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          h1: ({ children }) => (
            <h1
              className={`${PROSE} mt-[1.2em] mb-2 text-heading-lg font-semibold text-text-primary first:mt-0`}
            >
              {children}
            </h1>
          ),
          h2: ({ children }) => (
            <h2
              className={`${PROSE} mt-[1.2em] mb-2 text-heading font-semibold text-text-primary first:mt-0`}
            >
              {children}
            </h2>
          ),
          h3: ({ children }) => (
            <h3
              className={`${PROSE} mt-[1.2em] mb-2 text-emphasis font-semibold text-text-primary first:mt-0`}
            >
              {children}
            </h3>
          ),
          p: ({ children }) => <p className={`${PROSE} m-0 mb-[1em]`}>{children}</p>,
          // remark-gfm marks task lists with `contains-task-list`; those get
          // no bullet marker (the checkbox is the marker).
          ul: ({ children, className: listClassName }) => (
            <ul
              className={`${PROSE} m-0 mb-[1em] ${
                listClassName?.includes("contains-task-list")
                  ? "list-none pl-1 [&_input]:mr-1.5 [&_input]:align-middle"
                  : "list-disc pl-5"
              }`}
            >
              {children}
            </ul>
          ),
          ol: ({ children }) => (
            <ol className={`${PROSE} m-0 mb-[1em] list-decimal pl-5`}>{children}</ol>
          ),
          blockquote: ({ children }) => (
            <blockquote
              className={`${PROSE} m-0 mb-[1em] border-l-2 border-border pl-3 text-text-tertiary`}
            >
              {children}
            </blockquote>
          ),
          a: ({ children, href }) => (
            <a href={href} className="text-accent">
              {children}
            </a>
          ),
          code: ({ children, className: codeClassName }) => (
            <code
              className={`rounded-[4px] bg-bg-tertiary px-1 py-px font-mono text-small ${codeClassName ?? ""}`}
            >
              {children}
            </code>
          ),
          pre: ({ children }) => (
            <pre className="m-0 mb-[1em] overflow-x-auto rounded-sm bg-bg-tertiary px-3 py-2.5 [&_code]:bg-transparent [&_code]:p-0">
              {children}
            </pre>
          ),
          table: ({ children }) => (
            <div className="mb-[1em] overflow-x-auto">
              <table className="border-collapse text-small">{children}</table>
            </div>
          ),
          // `style` carries GFM column alignment (text-align from `:---:`
          // markers) - pass it through so centered/right columns survive.
          th: ({ children, style }) => (
            <th
              style={style}
              className="border border-border-subtle px-2.5 py-1.5 text-left font-semibold text-text-primary"
            >
              {children}
            </th>
          ),
          td: ({ children, style }) => (
            <td style={style} className="border border-border-subtle px-2.5 py-1.5 text-left">
              {children}
            </td>
          ),
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
