export const PLAN_CONTENT_LIMITS = {
  tasks: 128,
  techItems: 24,
  dependencyEdges: 512,
  dependencyInputs: 2_048,
  scopePathsPerTask: 24,
  labelCodePoints: 160,
  detailCodePoints: 1_024,
} as const;

const INPUT_BUDGET_FACTOR = 8;
const REMOVED_ELEMENTS = [
  "a",
  "abbr",
  "address",
  "area",
  "article",
  "aside",
  "audio",
  "b",
  "base",
  "bdi",
  "bdo",
  "big",
  "blockquote",
  "body",
  "br",
  "button",
  "canvas",
  "caption",
  "cite",
  "code",
  "col",
  "colgroup",
  "data",
  "datalist",
  "dd",
  "del",
  "details",
  "dfn",
  "dialog",
  "div",
  "dl",
  "dt",
  "embed",
  "em",
  "fieldset",
  "figcaption",
  "figure",
  "font",
  "footer",
  "form",
  "frame",
  "frameset",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "head",
  "header",
  "hgroup",
  "hr",
  "html",
  "i",
  "iframe",
  "img",
  "input",
  "ins",
  "kbd",
  "label",
  "legend",
  "li",
  "link",
  "main",
  "map",
  "mark",
  "menu",
  "meta",
  "meter",
  "nav",
  "noframes",
  "noscript",
  "object",
  "ol",
  "optgroup",
  "option",
  "output",
  "p",
  "param",
  "picture",
  "pre",
  "progress",
  "q",
  "rp",
  "rt",
  "ruby",
  "s",
  "samp",
  "script",
  "search",
  "section",
  "select",
  "small",
  "slot",
  "source",
  "span",
  "strike",
  "strong",
  "style",
  "sub",
  "summary",
  "sup",
  "svg",
  "table",
  "tbody",
  "td",
  "template",
  "textarea",
  "tfoot",
  "th",
  "thead",
  "time",
  "title",
  "tr",
  "track",
  "tt",
  "u",
  "ul",
  "var",
  "video",
  "wbr",
].join("|");

const LINE_BREAK_ELEMENT = /<br(?:\s+[^<>]*?)?\s*\/?>/giu;
const DISPLAY_ELEMENT = new RegExp(
  `<(\\/?)(${REMOVED_ELEMENTS})(?:\\s+[^<>]*?)?\\s*\\/?>`,
  "giu",
);
const HTML_ENTITY = /&(?:#(?:x[0-9a-f]+|\d+)|lt|gt|amp|quot|apos);/giu;

// Keep the zero-width joiner used by emoji and many writing systems, while
// removing controls and directional overrides that can disguise compact text.
const INVISIBLE_CONTROL =
  /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u00ad\u061c\u200b\u200e\u200f\u202a-\u202e\u2060\u2066-\u2069\ufeff]/g;

export function sanitizePlanLabel(value: unknown): string {
  return normalizePlanText(value, PLAN_CONTENT_LIMITS.labelCodePoints);
}

export function sanitizePlanDetail(value: unknown): string {
  return normalizePlanText(value, PLAN_CONTENT_LIMITS.detailCodePoints);
}

function normalizePlanText(value: unknown, limit: number): string {
  if (typeof value !== "string") return "";

  const bounded = value.slice(0, limit * INPUT_BUDGET_FACTOR);
  const validUnicode = Array.from(bounded)
    .filter((character) => {
      const point = character.codePointAt(0) ?? 0;
      return point < 0xd800 || point > 0xdfff;
    })
    .join("");
  const plainText = validUnicode
    .replace(INVISIBLE_CONTROL, "")
    .replace(LINE_BREAK_ELEMENT, " ")
    .trim();
  const withoutDisplayMarkup = stripPairedDisplayMarkup(plainText);
  const { text, markdownWrapped } = unwrapMatchingQuotes(withoutDisplayMarkup);
  const readableText = markdownWrapped ? removeMarkdownDelimiters(text) : text;
  const normalized = decodeEntities(readableText)
    .normalize("NFC")
    .replace(INVISIBLE_CONTROL, "")
    .replace(/\s+/g, " ")
    .trim();

  return truncateText(normalized, limit);
}

/**
 * Strip paired display markup while preserving angle-bracket tokens that are
 * commonly used as literal technical prose. Callers render the value as React
 * text or XML-escape it for SVG export, so an unmatched `<button>` remains
 * inert and visible rather than being mistaken for presentation markup.
 */
function stripPairedDisplayMarkup(value: string): string {
  const opened = new Set<string>();
  const closed = new Set<string>();
  for (const match of value.matchAll(DISPLAY_ELEMENT)) {
    const name = match[2]?.toLowerCase();
    if (!name) continue;
    if (match[1] === "/") closed.add(name);
    else opened.add(name);
  }

  const paired = new Set([...opened].filter((name) => closed.has(name)));
  DISPLAY_ELEMENT.lastIndex = 0;
  return value.replace(DISPLAY_ELEMENT, (element, _closing, name: string) =>
    paired.has(name.toLowerCase()) ? "" : element,
  );
}

function unwrapMatchingQuotes(value: string): {
  text: string;
  markdownWrapped: boolean;
} {
  const quote = value[0];
  const wrapped =
    value.length >= 2 &&
    (quote === '"' || quote === "'" || quote === "`") &&
    value[value.length - 1] === quote;

  return {
    text: wrapped ? value.slice(1, -1).trim() : value,
    markdownWrapped: wrapped && quote === "`",
  };
}

function removeMarkdownDelimiters(value: string): string {
  return value
    .replace(/`/g, "")
    .replace(/\*\*|__/g, "")
    .replace(/([\p{L}\p{N}])[*_]([\p{L}\p{N}])/gu, "$1$2")
    .trim();
}

function decodeEntities(value: string): string {
  return value.replace(HTML_ENTITY, (entity) => {
    const body = entity.slice(1, -1).toLowerCase();
    switch (body) {
      case "lt":
        return "<";
      case "gt":
        return ">";
      case "amp":
        return "&";
      case "quot":
        return '"';
      case "apos":
        return "'";
      default:
        return decodeNumericEntity(entity, body);
    }
  });
}

function decodeNumericEntity(entity: string, body: string): string {
  const hexadecimal = body.startsWith("#x");
  const digits = body.slice(hexadecimal ? 2 : 1);
  const point = Number.parseInt(digits, hexadecimal ? 16 : 10);
  if (
    !Number.isSafeInteger(point) ||
    point < 0 ||
    point > 0x10ffff ||
    (point >= 0xd800 && point <= 0xdfff)
  ) {
    return entity;
  }

  const decoded = String.fromCodePoint(point);
  INVISIBLE_CONTROL.lastIndex = 0;
  const hidden = INVISIBLE_CONTROL.test(decoded);
  INVISIBLE_CONTROL.lastIndex = 0;
  return hidden ? "" : decoded;
}

function truncateText(value: string, limit: number): string {
  const points = Array.from(value);
  if (points.length <= limit) return value;
  return `${points.slice(0, Math.max(0, limit - 1)).join("")}…`;
}
