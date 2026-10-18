import { expect, it } from "vitest";
import { fuseAuthorizedRankings } from "./hybrid-ranking.js";

it("fuses lexical and semantic rank without importing unknown or revoked records", () => {
  const allowed = new Map([["a", "source A"], ["b", "source B"]]);
  const rankings = [
    [{ id: "revoked", score: 100 }, { id: "a", score: 2 }, { id: "a", score: 1 }],
    [{ id: "b", score: 10 }, { id: "a", score: 9 }, { id: "unknown", score: 8 }],
  ];
  const result = fuseAuthorizedRankings(allowed, rankings);
  expect(result.map((row) => row.id)).toEqual(["a", "b"]);
  expect(result[0].rankings).toEqual([1, 2]);
  allowed.delete("a");
  expect(fuseAuthorizedRankings(allowed, rankings).map((row) => row.id)).toEqual(["b"]);
});

it("rejects unbounded options and ignores invalid provider scores", () => {
  const allowed = new Map([["a", 1]]);
  expect(fuseAuthorizedRankings(allowed, [[{ id: "a", score: NaN }]])).toEqual([]);
  expect(() => fuseAuthorizedRankings(allowed, [], { limit: Infinity })).toThrow();
  expect(() => fuseAuthorizedRankings(allowed, [], { rankConstant: 0 })).toThrow();
});
