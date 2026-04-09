import type { CanonicalSkillFrontmatter } from "../schema.js";

interface YamlObject {
  [key: string]: YamlValue;
}

type YamlValue = string | boolean | string[] | YamlObject;

interface Frame {
  indent: number;
  container: YamlObject;
}

function parseInlineArray(value: string): string[] {
  const inner = value.slice(1, -1).trim();
  if (!inner) return [];
  return inner
    .split(",")
    .map((item) => item.trim().replace(/^['"]|['"]$/g, ""))
    .filter(Boolean);
}

function parseScalar(value: string): string | boolean | string[] {
  const trimmed = value.trim();
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  if (trimmed.startsWith("[") && trimmed.endsWith("]")) {
    return parseInlineArray(trimmed);
  }
  return trimmed.replace(/^['"]|['"]$/g, "");
}

function nextMeaningfulLine(lines: string[], index: number): string | undefined {
  for (let i = index + 1; i < lines.length; i += 1) {
    const line = lines[i];
    if (line.trim().length === 0) continue;
    return line;
  }
  return undefined;
}

function stripInlineComment(line: string): string {
  const hashIndex = line.indexOf(" #");
  return hashIndex >= 0 ? line.slice(0, hashIndex) : line;
}

function setNestedValue(
  stack: Frame[],
  indent: number,
  key: string,
  value: YamlValue,
): void {
  while (stack.length > 1 && indent <= stack[stack.length - 1].indent) {
    stack.pop();
  }
  stack[stack.length - 1].container[key] = value;
  if (value && typeof value === "object" && !Array.isArray(value)) {
    stack.push({ indent, container: value });
  }
}

export function extractFrontmatter(source: string): {
  frontmatterRaw: string;
  body: string;
} {
  if (!source.startsWith("---\n")) {
    throw new Error("Canonical skill source is missing opening frontmatter delimiter");
  }
  const closingIndex = source.indexOf("\n---\n", 4);
  if (closingIndex === -1) {
    throw new Error("Canonical skill source is missing closing frontmatter delimiter");
  }
  return {
    frontmatterRaw: source.slice(4, closingIndex),
    body: source.slice(closingIndex + 5),
  };
}

export function parseFrontmatter(raw: string): CanonicalSkillFrontmatter {
  const root: YamlObject = {};
  const stack: Frame[] = [{ indent: -1, container: root }];
  const lines = raw.split("\n");

  for (let i = 0; i < lines.length; i += 1) {
    const original = lines[i];
    const line = stripInlineComment(original).replace(/\r$/, "");
    if (line.trim().length === 0) continue;

    const indent = line.match(/^ */)?.[0].length ?? 0;
    const trimmed = line.trim();

    if (trimmed.startsWith("- ")) {
      throw new Error(
        `Top-level sequence syntax is not supported in canonical frontmatter yet: "${trimmed}"`,
      );
    }

    const colonIndex = trimmed.indexOf(":");
    if (colonIndex === -1) {
      throw new Error(`Invalid frontmatter line: "${trimmed}"`);
    }

    const key = trimmed.slice(0, colonIndex).trim();
    const valuePart = trimmed.slice(colonIndex + 1).trim();

    if (valuePart.length > 0) {
      setNestedValue(stack, indent, key, parseScalar(valuePart));
      continue;
    }

    const nextLine = nextMeaningfulLine(lines, i);
    if (nextLine?.trim().startsWith("- ")) {
      const items: string[] = [];
      while (i + 1 < lines.length) {
        const candidate = lines[i + 1].replace(/\r$/, "");
        if (candidate.trim().length === 0) {
          i += 1;
          continue;
        }
        const candidateIndent = candidate.match(/^ */)?.[0].length ?? 0;
        const candidateTrimmed = candidate.trim();
        if (candidateIndent <= indent || !candidateTrimmed.startsWith("- ")) {
          break;
        }
        items.push(parseScalar(candidateTrimmed.slice(2)) as string);
        i += 1;
      }
      setNestedValue(stack, indent, key, items);
      continue;
    }

    setNestedValue(stack, indent, key, {});
  }

  return root as unknown as CanonicalSkillFrontmatter;
}
