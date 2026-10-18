/** Literal lexical retrieval shared by SDK and MCP. No I/O or authorization. */
export const RETRIEVAL_CONTRACT = "literal-and-prefix-v1";
export const SEARCH_QUERY_MAX_CHARS = 2048;

export function foldSearchText(value: string): string {
  return Array.from(value.normalize("NFD").replace(/\p{M}/gu, ""), (character) => character.toLowerCase()).join("");
}

export function searchTokens(value: string): string[] {
  return foldSearchText(value).match(/[\p{L}\p{N}]+/gu) ?? [];
}

export function queryTokens(query: string): string[] {
  if (query.length > SEARCH_QUERY_MAX_CHARS) {
    throw new RangeError(`Search query exceeds ${SEARCH_QUERY_MAX_CHARS} characters`);
  }
  return searchTokens(query);
}

export interface LexicalMatch {
  /** Local lexical relevance; not a probability or a cross-engine BM25 score. */
  score: number;
  contract: typeof RETRIEVAL_CONTRACT;
}

export function matchesSearchToken(token: string, wanted: string, final: boolean): boolean {
  return final && [...wanted].length >= 2 ? token.startsWith(wanted) : token === wanted;
}

/** AND across title/body; only the final >=2-codepoint token has prefix matching. */
export function lexicalMatch(title: string, body: string, query: string): LexicalMatch | null {
  const wanted = queryTokens(query);
  if (wanted.length === 0) return null;
  const titleTokens = searchTokens(title);
  const bodyTokens = searchTokens(body);
  let score = 0;
  for (let index = 0; index < wanted.length; index += 1) {
    const matches = (token: string) => matchesSearchToken(token, wanted[index], index === wanted.length - 1);
    const titleCount = titleTokens.filter(matches).length;
    const bodyCount = bodyTokens.filter(matches).length;
    if (titleCount + bodyCount === 0) return null;
    score += 4 * Math.log2(1 + titleCount) + Math.log2(1 + bodyCount);
  }
  const phrase = wanted.join(" ");
  if (titleTokens.join(" ").includes(phrase)) score += 4;
  if (bodyTokens.join(" ").includes(phrase)) score += 1;
  return { score, contract: RETRIEVAL_CONTRACT };
}

export function compareSearchMatches(
  left: { score: number; date: string; path: string },
  right: { score: number; date: string; path: string }
): number {
  const date = (value: string) => Number.isFinite(Date.parse(value)) ? Date.parse(value) : 0;
  return right.score - left.score || date(right.date) - date(left.date) ||
    (left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
}
