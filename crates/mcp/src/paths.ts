import { existsSync, realpathSync } from "fs";
import { homedir } from "os";
import { extname, join, resolve } from "path";

export function expandHomeLikePath(input: string): string {
  const home = homedir();

  if (input === "~") {
    return home;
  }

  if (input.startsWith("~/") || input.startsWith("~\\")) {
    return join(home, input.slice(2));
  }

  if (input === "$HOME" || input.startsWith("$HOME/") || input.startsWith("$HOME\\")) {
    return home + input.slice("$HOME".length);
  }

  if (input === "${HOME}" || input.startsWith("${HOME}/") || input.startsWith("${HOME}\\")) {
    return home + input.slice("${HOME}".length);
  }

  return input;
}

export function canonicalizeFilePath(path: string): string {
  if (!existsSync(path)) {
    throw new Error(`Path does not exist: ${path}`);
  }
  return realpathSync(path);
}

export function canonicalizeRoot(root: string): string {
  const expandedRoot = expandHomeLikePath(root);

  // Roots may not exist yet (e.g. ~/.minutes/inbox on first run).
  // Use realpath if it exists, otherwise lexical resolve.
  return existsSync(expandedRoot) ? realpathSync(expandedRoot) : resolve(expandedRoot);
}

export function isWithinDirectory(candidate: string, root: string): boolean {
  // Ensure root ends with separator to prevent prefix attacks (e.g. ~/meetings-evil)
  const rootWithSep = root.endsWith("/") ? root : root + "/";
  return candidate === root || candidate.startsWith(rootWithSep);
}

export function validatePathInDirectory(path: string, root: string, allowedExts: string[]): string {
  const canonicalPath = canonicalizeFilePath(path);
  const canonicalRoot = canonicalizeRoot(root);

  if (!allowedExts.includes(extname(canonicalPath).toLowerCase())) {
    throw new Error(
      `Access denied: path must be within ${canonicalRoot} and end with ${allowedExts.join(", ")}`
    );
  }

  if (!isWithinDirectory(canonicalPath, canonicalRoot)) {
    throw new Error(`Access denied: path must be within ${canonicalRoot}`);
  }

  return canonicalPath;
}

export function validatePathInDirectories(
  path: string,
  roots: string[],
  allowedExts: string[]
): string {
  const canonicalPath = canonicalizeFilePath(path);

  if (!allowedExts.includes(extname(canonicalPath).toLowerCase())) {
    throw new Error(
      `Access denied: path must end with one of ${allowedExts.join(", ")}`
    );
  }

  const canonicalRoots = roots.map((root) => canonicalizeRoot(root));
  if (!canonicalRoots.some((root) => isWithinDirectory(canonicalPath, root))) {
    throw new Error(
      `Access denied: file must be inside one of ${canonicalRoots.join(", ")}`
    );
  }

  return canonicalPath;
}
