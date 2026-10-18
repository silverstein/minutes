/**
 * Experimental rank fusion for evaluations and local semantic adapters.
 * Callers must supply candidates from their CURRENT authorized snapshot and
 * revalidate that snapshot before publishing. This function grants no access.
 * There is no model, filesystem access, persistence or remote provider here.
 */
export interface RankedCandidate { id: string; score: number }
export interface FusedCandidate<T> { id: string; candidate: T; score: number; rankings: number[] }

export function fuseAuthorizedRankings<T>(
  authorizedCandidates: ReadonlyMap<string, T>,
  rankings: readonly (readonly RankedCandidate[])[],
  options: { limit?: number; rankConstant?: number } = {}
): FusedCandidate<T>[] {
  const limit = options.limit ?? 20;
  const rankConstant = options.rankConstant ?? 60;
  if (!Number.isInteger(limit) || limit < 1 || limit > 1000) throw new RangeError("Fusion limit must be between 1 and 1000");
  if (!Number.isFinite(rankConstant) || rankConstant < 1) throw new RangeError("Fusion rank constant must be finite and positive");
  if (rankings.length > 16 || rankings.some((ranking) => ranking.length > 10_000)) throw new RangeError("Fusion ranking budget exceeded");
  const fused = new Map<string, FusedCandidate<T>>();
  for (const ranking of rankings) {
    const seen = new Set<string>();
    // External scores order candidates only; they never enter fused-score math.
    const eligible = ranking.filter(({ id, score }) => authorizedCandidates.has(id) && Number.isFinite(score))
      .sort((a, b) => b.score - a.score || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
    let rank = 0;
    for (const { id } of eligible) {
      if (seen.has(id)) continue;
      seen.add(id);
      rank += 1;
      const item = fused.get(id) ?? { id, candidate: authorizedCandidates.get(id)!, score: 0, rankings: [] };
      item.score += 1 / (rankConstant + rank);
      item.rankings.push(rank);
      fused.set(id, item);
    }
  }
  return [...fused.values()].sort((a, b) => b.score - a.score || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)).slice(0, limit);
}
