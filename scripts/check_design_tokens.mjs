#!/usr/bin/env node

/**
 * Design-token enforcement invariant. These are the only syntax positions in
 * which this checker interprets a literal as a design color or font:
 *
 * (a) CSS declarations in .css files, <style> blocks, and style="" attributes:
 *     color, background*, border*-color, fill, stroke, outline-color,
 *     box-shadow colors, font-family, and token custom properties.
 * (b) TS/TSX style-object values whose keys are color, background*,
 *     border*Color, fill, stroke, or fontFamily.
 * (c) Tailwind arbitrary color classes such as text-[#...] and bg-[rgb(...)].
 * (d) Literal JSX/SVG fill=, stroke=, and color= attributes.
 *
 * The scanner deliberately does not grep prose. Raw sanctioned values are
 * accepted only in TOKEN_DEFINITION_FILES; all other findings are compared to
 * the exact, line-agnostic shrink-only baseline.
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const SCAN_ROOTS = ["site", "tauri/src"];
const TOKEN_DEFINITION_FILES = new Set([
  "site/app/globals.css",
  "tauri/src/index.html",
  "tauri/src/styles/theme-tokens.css",
]);
const SOURCE_EXTENSIONS = new Set([".css", ".html", ".js", ".jsx", ".ts", ".tsx"]);
const CSS_NAMED_COLORS = new Set([
  "aliceblue", "antiquewhite", "aqua", "aquamarine", "azure", "beige", "bisque",
  "black", "blanchedalmond", "blue", "blueviolet", "brown", "burlywood", "cadetblue",
  "chartreuse", "chocolate", "coral", "cornflowerblue", "cornsilk", "crimson", "cyan",
  "darkblue", "darkcyan", "darkgoldenrod", "darkgray", "darkgreen", "darkgrey", "darkkhaki",
  "darkmagenta", "darkolivegreen", "darkorange", "darkorchid", "darkred", "darksalmon",
  "darkseagreen", "darkslateblue", "darkslategray", "darkslategrey", "darkturquoise",
  "darkviolet", "deeppink", "deepskyblue", "dimgray", "dimgrey", "dodgerblue",
  "firebrick", "floralwhite", "forestgreen", "fuchsia", "gainsboro", "ghostwhite", "gold",
  "goldenrod", "gray", "green", "greenyellow", "grey", "honeydew", "hotpink", "indianred",
  "indigo", "ivory", "khaki", "lavender", "lavenderblush", "lawngreen", "lemonchiffon",
  "lightblue", "lightcoral", "lightcyan", "lightgoldenrodyellow", "lightgray", "lightgreen",
  "lightgrey", "lightpink", "lightsalmon", "lightseagreen", "lightskyblue", "lightslategray",
  "lightslategrey", "lightsteelblue", "lightyellow", "lime", "limegreen", "linen", "magenta",
  "maroon", "mediumaquamarine", "mediumblue", "mediumorchid", "mediumpurple",
  "mediumseagreen", "mediumslateblue", "mediumspringgreen", "mediumturquoise",
  "mediumvioletred", "midnightblue", "mintcream", "mistyrose", "moccasin", "navajowhite",
  "navy", "oldlace", "olive", "olivedrab", "orange", "orangered", "orchid",
  "palegoldenrod", "palegreen", "paleturquoise", "palevioletred", "papayawhip",
  "peachpuff", "peru", "pink", "plum", "powderblue", "purple", "rebeccapurple", "red",
  "rosybrown", "royalblue", "saddlebrown", "salmon", "sandybrown", "seagreen", "seashell",
  "sienna", "silver", "skyblue", "slateblue", "slategray", "slategrey", "snow",
  "springgreen", "steelblue", "tan", "teal", "thistle", "tomato", "turquoise", "violet",
  "wheat", "white", "whitesmoke", "yellow", "yellowgreen",
]);

function usage(message) {
  const suffix = message ? `\nError: ${message}` : "";
  return `Usage: node scripts/check_design_tokens.mjs [--root DIR] [--base-baseline FILE] [--write-baseline] [--json]${suffix}`;
}

function parseArguments(argv) {
  let root;
  let baseBaseline;
  let writeBaseline = false;
  let json = false;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--root" || argument === "--base-baseline") {
      if (index + 1 >= argv.length) throw new Error(usage(`${argument} requires a value`));
      const value = argv[index + 1];
      if (argument === "--root") root = value;
      else baseBaseline = value;
      index += 1;
    } else if (argument === "--write-baseline") {
      writeBaseline = true;
    } else if (argument === "--json") {
      json = true;
    } else {
      throw new Error(usage(`unknown argument: ${argument}`));
    }
  }
  const defaultRoot = path.resolve(path.dirname(process.argv[1]), "..");
  return {
    root: path.resolve(root ?? defaultRoot),
    baseBaseline: baseBaseline ? path.resolve(baseBaseline) : null,
    writeBaseline,
    json,
  };
}

function normalizePath(file) {
  return file.split(path.sep).join("/");
}

function normalizeNumber(number) {
  const parsed = Number(number);
  return Number.isFinite(parsed) ? String(parsed) : number;
}

function normalizeColor(value) {
  const trimmed = value.trim();
  if (trimmed.startsWith("#")) {
    const digits = trimmed.slice(1).toLowerCase();
    if (digits.length === 3 || digits.length === 4) {
      return `#${[...digits].map((digit) => digit + digit).join("")}`;
    }
    return `#${digits}`;
  }
  if (/^[a-z]+$/i.test(trimmed)) return trimmed.toLowerCase();
  return trimmed
    .toLowerCase()
    .replace(/[-+]?(?:\d*\.\d+|\d+\.?\d*)/g, normalizeNumber)
    .replace(/\s+/g, "");
}

function findClosingParenthesis(text, openingIndex) {
  let depth = 0;
  let quote = null;
  for (let index = openingIndex; index < text.length; index += 1) {
    const character = text[index];
    if (quote) {
      if (character === "\\") index += 1;
      else if (character === quote) quote = null;
      continue;
    }
    if (character === "'" || character === '"') quote = character;
    else if (character === "(") depth += 1;
    else if (character === ")" && --depth === 0) return index;
  }
  return -1;
}

function extractColorLiterals(text) {
  const literals = [];
  let plain = "";
  const flushPlain = () => {
    for (const match of plain.matchAll(/#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{4}|[0-9a-f]{3})(?![0-9a-f])|\b[a-z]+\b/gi)) {
      const value = match[0];
      if (value.startsWith("#") || CSS_NAMED_COLORS.has(value.toLowerCase())) literals.push(value);
    }
    plain = "";
  };

  for (let index = 0; index < text.length; index += 1) {
    const identifier = /^[a-z][a-z0-9-]*/i.exec(text.slice(index));
    if (identifier && text[index + identifier[0].length] === "(") {
      flushPlain();
      const name = identifier[0].toLowerCase();
      const opening = index + identifier[0].length;
      const closing = findClosingParenthesis(text, opening);
      if (closing === -1) {
        plain += text[index];
        continue;
      }
      const whole = text.slice(index, closing + 1);
      if (["rgb", "rgba", "hsl", "hsla", "oklch"].includes(name)) literals.push(whole);
      else if (name !== "var" && name !== "url") {
        literals.push(...extractColorLiterals(text.slice(opening + 1, closing)));
      }
      index = closing;
    } else {
      plain += text[index];
    }
  }
  flushPlain();
  return literals;
}

function stripComments(source, html = false, lineComments = true) {
  const output = [...source];
  let quote = null;
  for (let index = 0; index < source.length; index += 1) {
    if (quote) {
      if (source[index] === "\\") index += 1;
      else if (source[index] === quote) quote = null;
      continue;
    }
    if (source[index] === "'" || source[index] === '"' || source[index] === "`") {
      quote = source[index];
      continue;
    }
    const isHtmlComment = html && source.startsWith("<!--", index);
    const isLineComment = lineComments && source.startsWith("//", index);
    const isBlockComment = source.startsWith("/*", index);
    if (!isHtmlComment && !isLineComment && !isBlockComment) continue;
    const terminator = isHtmlComment ? "-->" : isLineComment ? "\n" : "*/";
    const end = source.indexOf(terminator, index + 2);
    const stop = end === -1 ? source.length : end + (isLineComment ? 0 : terminator.length);
    for (let cursor = index; cursor < stop; cursor += 1) {
      if (output[cursor] !== "\n" && output[cursor] !== "\r") output[cursor] = " ";
    }
    index = stop - 1;
  }
  return output.join("");
}

function splitCssDeclarations(text) {
  const declarations = [];
  let start = 0;
  let quote = null;
  let parentheses = 0;
  for (let index = 0; index <= text.length; index += 1) {
    const character = text[index];
    if (quote) {
      if (character === "\\") index += 1;
      else if (character === quote) quote = null;
      continue;
    }
    if (character === "'" || character === '"') quote = character;
    else if (character === "(") parentheses += 1;
    else if (character === ")") parentheses = Math.max(0, parentheses - 1);
    else if ((character === ";" || index === text.length) && parentheses === 0) {
      declarations.push(text.slice(start, index));
      start = index + 1;
    }
  }
  return declarations;
}

function cssBlocks(source) {
  const blocks = [];
  function walk(start, stopCharacter = null) {
    let segmentStart = start;
    let quote = null;
    let parentheses = 0;
    for (let index = start; index < source.length; index += 1) {
      const character = source[index];
      if (quote) {
        if (character === "\\") index += 1;
        else if (character === quote) quote = null;
        continue;
      }
      if (character === "'" || character === '"') quote = character;
      else if (character === "(") parentheses += 1;
      else if (character === ")") parentheses = Math.max(0, parentheses - 1);
      else if (parentheses === 0 && character === "{") {
        const closing = walk(index + 1, "}");
        index = closing;
        segmentStart = index + 1;
      } else if (parentheses === 0 && character === stopCharacter) {
        blocks.push(source.slice(segmentStart, index));
        return index;
      }
    }
    return source.length;
  }
  walk(0);
  return blocks;
}

function propertyCarriesColor(property) {
  return property === "color" || property.startsWith("background") ||
    (/^border(?:-[a-z0-9]+)*-color$/.test(property)) || property === "fill" ||
    property === "stroke" || property === "outline-color" || property === "box-shadow";
}

function styleObjectKeyCarriesColor(key) {
  return key === "color" || key.startsWith("background") ||
    (/^border.*Color$/.test(key)) || key === "fill" || key === "stroke";
}

function parseFontFamilies(value) {
  return value.split(",").map((family) => family.trim().replace(/^(['"])(.*)\1$/, "$2"))
    .filter((family) => family && !/^var\(/i.test(family));
}

function readJson(file, description) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    throw new Error(`cannot read ${description} ${file}: ${error.message}`);
  }
}

function validateTokens(tokens) {
  if (!tokens || typeof tokens.colors !== "object" || Array.isArray(tokens.colors)) {
    throw new Error("design/tokens.json must contain a colors object");
  }
  if (!Array.isArray(tokens.fonts) || !Array.isArray(tokens.allowedKeywords)) {
    throw new Error("design/tokens.json must contain fonts and allowedKeywords arrays");
  }
  for (const [name, value] of Object.entries(tokens.colors)) {
    if (typeof value !== "string" || extractColorLiterals(value).length !== 1) {
      throw new Error(`design token color ${name} must be one color literal`);
    }
  }
  if (![...tokens.fonts, ...tokens.allowedKeywords].every((value) => typeof value === "string")) {
    throw new Error("font and allowed-keyword tokens must be strings");
  }
}

function walkFiles(root) {
  const files = [];
  for (const relativeRoot of SCAN_ROOTS) {
    const absoluteRoot = path.join(root, relativeRoot);
    if (!fs.existsSync(absoluteRoot)) continue;
    const queue = [absoluteRoot];
    while (queue.length > 0) {
      const directory = queue.pop();
      for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
        const absolute = path.join(directory, entry.name);
        if (entry.isDirectory()) queue.push(absolute);
        else if (entry.isFile() && SOURCE_EXTENSIONS.has(path.extname(entry.name))) files.push(absolute);
      }
    }
  }
  return files.sort();
}

function collectViolations(root, tokens) {
  const tokenColors = new Set(Object.values(tokens.colors).map(normalizeColor));
  const tokenFonts = new Set(tokens.fonts.map((font) => font.toLowerCase()));
  const allowedKeywords = new Set(tokens.allowedKeywords.map((keyword) => keyword.toLowerCase()));
  const entries = new Map();

  function add(file, property, value) {
    const entry = { file, property, value: value.trim().replace(/\s+/g, " ") };
    entries.set(JSON.stringify(entry), entry);
  }

  function checkColors(file, property, value) {
    const isDefinition = TOKEN_DEFINITION_FILES.has(file);
    for (const literal of extractColorLiterals(value)) {
      const normalized = normalizeColor(literal);
      if (allowedKeywords.has(normalized)) continue;
      if (!isDefinition || !tokenColors.has(normalized)) add(file, property, literal);
    }
  }

  function checkFont(file, property, value) {
    const normalizedValue = value.trim().toLowerCase();
    if (allowedKeywords.has(normalizedValue) || /^var\([^)]*\)$/.test(normalizedValue)) return;
    const families = parseFontFamilies(value);
    if (families.length === 0) return;
    const isDefinition = TOKEN_DEFINITION_FILES.has(file);
    if (!isDefinition || families.some((family) => !tokenFonts.has(family.toLowerCase()))) {
      add(file, property, value);
    }
  }

  function checkDeclaration(file, declaration) {
    const colon = declaration.indexOf(":");
    if (colon === -1) return;
    const property = declaration.slice(0, colon).trim().toLowerCase();
    const value = declaration.slice(colon + 1).trim().replace(/\s*!important\s*$/i, "");
    if (!property || !value) return;
    if (property.startsWith("--")) {
      checkColors(file, property, value);
      if (property.startsWith("--font")) checkFont(file, property, value);
    } else if (property === "font-family") {
      checkFont(file, property, value);
    } else if (propertyCarriesColor(property)) {
      checkColors(file, property, value);
    }
  }

  function checkCss(file, css, declarationsOnly = false) {
    const stripped = stripComments(css, false, false);
    const blocks = declarationsOnly ? [stripped] : cssBlocks(stripped);
    for (const block of blocks) {
      for (const declaration of splitCssDeclarations(block)) checkDeclaration(file, declaration);
    }
  }

  function checkTailwind(file, source) {
    const pattern =
      /\b(text|bg|border|fill|stroke|outline|shadow|ring|ring-offset|caret|accent|divide|decoration|placeholder|from|via|to)-\[([^\]\r\n]+)\]/gi;
    for (const match of source.matchAll(pattern)) {
      const literals = extractColorLiterals(match[2]);
      for (const literal of literals) {
        if (!allowedKeywords.has(normalizeColor(literal))) add(file, match[1].toLowerCase(), literal);
      }
    }
  }

  function jsTokens(source) {
    const result = [];
    for (let index = 0; index < source.length;) {
      if (/\s/.test(source[index])) { index += 1; continue; }
      if (source.startsWith("//", index)) {
        index = source.indexOf("\n", index + 2);
        if (index === -1) break;
        continue;
      }
      if (source.startsWith("/*", index)) {
        const end = source.indexOf("*/", index + 2);
        index = end === -1 ? source.length : end + 2;
        continue;
      }
      const character = source[index];
      if (character === "'" || character === '"' || character === "`") {
        const quote = character;
        let cursor = index + 1;
        let value = "";
        while (cursor < source.length) {
          if (source[cursor] === "\\") {
            value += source.slice(cursor, cursor + 2);
            cursor += 2;
          } else if (source[cursor] === quote) {
            cursor += 1;
            break;
          } else {
            value += source[cursor++];
          }
        }
        result.push({ type: "string", value });
        index = cursor;
        continue;
      }
      const identifier = /^[A-Za-z_$][\w$-]*/.exec(source.slice(index));
      if (identifier) {
        result.push({ type: "identifier", value: identifier[0] });
        index += identifier[0].length;
      } else {
        result.push({ type: "punctuation", value: character });
        index += 1;
      }
    }
    return result;
  }

  function checkStyleObjects(file, source) {
    const tokensList = jsTokens(source);
    for (let index = 1; index + 2 < tokensList.length; index += 1) {
      const key = tokensList[index];
      const previous = tokensList[index - 1];
      const colon = tokensList[index + 1];
      const value = tokensList[index + 2];
      if (!["{", ","].includes(previous.value) || colon.value !== ":" || value.type !== "string") continue;
      if (key.value === "fontFamily") checkFont(file, key.value, value.value);
      else if (styleObjectKeyCarriesColor(key.value)) checkColors(file, key.value, value.value);
    }
  }

  function checkTagAttributes(file, source, includeStyle) {
    const withoutComments = stripComments(source, true);
    for (const tag of withoutComments.matchAll(/<[A-Za-z][^<>]*>/gs)) {
      const text = tag[0];
      const attributePattern = /(?:^|\s)(fill|stroke|color)\s*=\s*(?:"([^"]*)"|'([^']*)'|\{\s*(["'`])(.*?)\4\s*\})/gi;
      for (const match of text.matchAll(attributePattern)) {
        const value = match[2] ?? match[3] ?? match[5] ?? "";
        checkColors(file, match[1].toLowerCase(), value);
      }
      if (includeStyle) {
        const stylePattern = /(?:^|\s)style\s*=\s*(?:"([^"]*)"|'([^']*)')/gi;
        for (const match of text.matchAll(stylePattern)) checkCss(file, match[1] ?? match[2], true);
      }
    }
  }

  for (const absolute of walkFiles(root)) {
    const file = normalizePath(path.relative(root, absolute));
    const extension = path.extname(file);
    const source = fs.readFileSync(absolute, "utf8");
    if (extension === ".css") checkCss(file, source);
    if (extension === ".html") {
      const withoutComments = stripComments(source, true);
      for (const match of withoutComments.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style\s*>/gi)) {
        checkCss(file, match[1]);
      }
      checkTagAttributes(file, source, true);
    }
    if ([".ts", ".tsx"].includes(extension)) checkStyleObjects(file, source);
    if ([".html", ".jsx", ".tsx"].includes(extension)) checkTagAttributes(file, source, false);
    checkTailwind(file, stripComments(source, extension === ".html"));
  }

  return [...entries.values()].sort((left, right) =>
    left.file.localeCompare(right.file) || left.property.localeCompare(right.property) ||
    left.value.localeCompare(right.value));
}

function validateBaseline(value, description) {
  if (!Array.isArray(value)) throw new Error(`${description} must be a JSON array`);
  const entries = value.map((entry, index) => {
    if (!entry || typeof entry.file !== "string" || typeof entry.property !== "string" ||
        typeof entry.value !== "string" || Object.keys(entry).length !== 3) {
      throw new Error(`${description} entry ${index} must contain only string file, property, and value fields`);
    }
    return entry;
  });
  const keys = entries.map(JSON.stringify);
  if (new Set(keys).size !== keys.length) throw new Error(`${description} contains duplicate entries`);
  return entries;
}

function compareEntries(expected, actual) {
  const expectedKeys = new Set(expected.map(JSON.stringify));
  const actualKeys = new Set(actual.map(JSON.stringify));
  return {
    missing: expected.filter((entry) => !actualKeys.has(JSON.stringify(entry))),
    unexpected: actual.filter((entry) => !expectedKeys.has(JSON.stringify(entry))),
  };
}

function renderEntry(entry) {
  return `${entry.file} [${entry.property}] ${entry.value}`;
}

function main() {
  const options = parseArguments(process.argv.slice(2));
  const tokensFile = path.join(options.root, "design/tokens.json");
  const baselineFile = path.join(options.root, "design/token-baseline.json");
  const tokens = readJson(tokensFile, "token contract");
  validateTokens(tokens);
  const current = collectViolations(options.root, tokens);

  if (options.writeBaseline) {
    fs.mkdirSync(path.dirname(baselineFile), { recursive: true });
    fs.writeFileSync(baselineFile, `${JSON.stringify(current, null, 2)}\n`);
  }

  const baseline = validateBaseline(readJson(baselineFile, "baseline"), "design/token-baseline.json");
  const equality = compareEntries(baseline, current);
  let additions = [];
  if (options.baseBaseline) {
    const base = validateBaseline(readJson(options.baseBaseline, "base baseline"), "base baseline");
    additions = compareEntries(base, baseline).unexpected;
  }
  const ok = equality.missing.length === 0 && equality.unexpected.length === 0 && additions.length === 0;
  const report = {
    ok,
    violations: current.length,
    staleBaselineEntries: equality.missing,
    unbaselinedViolations: equality.unexpected,
    addedBaselineEntries: additions,
  };

  if (options.json) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  } else if (ok) {
    process.stdout.write(`Design tokens: ${current.length} current violations exactly match the shrink-only baseline.\n`);
  } else {
    process.stderr.write("Design token check failed.\n");
    if (equality.unexpected.length) {
      process.stderr.write("Unbaselined violations:\n");
      for (const entry of equality.unexpected) process.stderr.write(`  + ${renderEntry(entry)}\n`);
    }
    if (equality.missing.length) {
      process.stderr.write("Stale baseline entries (remove these as violations burn down):\n");
      for (const entry of equality.missing) process.stderr.write(`  - ${renderEntry(entry)}\n`);
    }
    if (additions.length) {
      process.stderr.write("Baseline additions are forbidden; the baseline may only shrink:\n");
      for (const entry of additions) process.stderr.write(`  + ${renderEntry(entry)}\n`);
    }
  }
  process.exitCode = ok ? 0 : 1;
}

try {
  main();
} catch (error) {
  process.stderr.write(`Design token check failed: ${error.message}\n`);
  process.exitCode = 1;
}
