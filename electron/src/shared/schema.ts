export const TRANSFER_EVENT_SCHEMA_VERSION = "1.0.0";

export type SchemaCompatibility = "compatible" | "mismatch" | "unknown";

function parseMajor(version: string): number | null {
  const match = /^(\d+)(?:\.\d+)?(?:\.\d+)?(?:[-+].*)?$/.exec(version.trim());
  if (!match || !match[1]) {
    return null;
  }
  const value = Number(match[1]);
  return Number.isFinite(value) ? value : null;
}

export function evaluateSchemaCompatibility(
  expectedVersion: string,
  actualVersion?: string | null
): SchemaCompatibility {
  const normalizedActual = typeof actualVersion === "string" ? actualVersion.trim() : "";
  if (!normalizedActual) {
    return "unknown";
  }

  const expectedMajor = parseMajor(expectedVersion);
  const actualMajor = parseMajor(normalizedActual);

  if (expectedMajor !== null && actualMajor !== null) {
    return expectedMajor === actualMajor ? "compatible" : "mismatch";
  }
  return expectedVersion.trim() === normalizedActual ? "compatible" : "mismatch";
}
