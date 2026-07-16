#!/usr/bin/env node

import { readFileSync, readdirSync, statSync } from "node:fs";
import { extname, join } from "node:path";

const DEFAULT_WORKFLOW_DIRECTORY = ".github/workflows";
const FORBIDDEN_RUNNER_CONTEXT =
  /\benv\s*(?:\.\s*(ImageOS|ImageVersion)\b|\[\s*(['"])(ImageOS|ImageVersion)\2\s*\])/g;

function stripYamlComments(source) {
  return source
    .split("\n")
    .map((line) => {
      let inSingleQuote = false;
      let inDoubleQuote = false;
      let escaped = false;

      for (let index = 0; index < line.length; index += 1) {
        const character = line[index];

        if (inDoubleQuote) {
          if (escaped) {
            escaped = false;
          } else if (character === "\\") {
            escaped = true;
          } else if (character === '"') {
            inDoubleQuote = false;
          }
          continue;
        }

        if (inSingleQuote) {
          if (character === "'" && line[index + 1] === "'") {
            index += 1;
          } else if (character === "'") {
            inSingleQuote = false;
          }
          continue;
        }

        if (character === '"') {
          inDoubleQuote = true;
        } else if (character === "'") {
          inSingleQuote = true;
        } else if (
          character === "#" &&
          (index === 0 || /\s/.test(line[index - 1]))
        ) {
          return `${line.slice(0, index)}${" ".repeat(line.length - index)}`;
        }
      }

      return line;
    })
    .join("\n");
}

function workflowFiles(paths) {
  const files = [];

  for (const path of paths) {
    const metadata = statSync(path);
    if (metadata.isDirectory()) {
      for (const entry of readdirSync(path, { withFileTypes: true })) {
        const entryPath = join(path, entry.name);
        if (entry.isDirectory()) {
          files.push(...workflowFiles([entryPath]));
        } else if ([".yml", ".yaml"].includes(extname(entry.name))) {
          files.push(entryPath);
        }
      }
    } else {
      files.push(path);
    }
  }

  return files.sort();
}

function* expressionSpans(source) {
  let searchOffset = 0;

  while (searchOffset < source.length) {
    const start = source.indexOf("${{", searchOffset);
    if (start === -1) {
      return;
    }

    let end;
    let inSingleQuote = false;
    for (let index = start + 3; index < source.length - 1; index += 1) {
      if (source[index] === "'") {
        if (inSingleQuote && source[index + 1] === "'") {
          index += 1;
        } else {
          inSingleQuote = !inSingleQuote;
        }
      } else if (
        !inSingleQuote &&
        source[index] === "}" &&
        source[index + 1] === "}"
      ) {
        end = index + 2;
        break;
      }
    }

    if (end === undefined) {
      return;
    }

    yield { expression: source.slice(start, end), index: start };
    searchOffset = end;
  }
}

function lineAndColumn(source, offset) {
  const precedingText = source.slice(0, offset);
  const lines = precedingText.split("\n");
  return { line: lines.length, column: lines.at(-1).length + 1 };
}

const paths = process.argv.slice(2);
const files = workflowFiles(
  paths.length > 0 ? paths : [DEFAULT_WORKFLOW_DIRECTORY],
);
let failed = false;

for (const file of files) {
  const source = readFileSync(file, "utf8");
  const uncommentedSource = stripYamlComments(source);

  for (const expressionMatch of expressionSpans(uncommentedSource)) {
    const { expression } = expressionMatch;
    for (const contextMatch of expression.matchAll(FORBIDDEN_RUNNER_CONTEXT)) {
      const offset = expressionMatch.index + contextMatch.index;
      const location = lineAndColumn(uncommentedSource, offset);
      const property = contextMatch[1] ?? contextMatch[3];
      console.error(
        `${file}:${location.line}:${location.column}: expressions cannot read env.${property}; ` +
          "read ${ImageOS} in a shell step and export a step output",
      );
      failed = true;
    }
  }
}

if (failed) {
  process.exitCode = 1;
}
