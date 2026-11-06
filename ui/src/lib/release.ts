/**
 * Which app releases wrote a save of a given data format, for a save that carries no record
 * of its app version (earlier releases did not keep one). Display only: it never decides
 * whether a save can be opened. The formats are those of the published releases; a format
 * that no release wrote has no range.
 *
 * Format 9 is both the request series (1.0.0 to 1.0.3, which kept no record) and 1.1.1, so
 * without a record it names both.
 */
const RANGES: Record<number, string> = {
  7: "1.0.4–1.1.0",
  8: "1.1.1",
  9: "1.0.0–1.0.3, 1.1.1",
};

export function releaseRange(schema: number): string | null {
  return RANGES[schema] ?? null;
}
