import type { SessionFailureDto } from "../bindings";

/** Narrows a command rejection into a SessionFailureDto. */
export function toFailure(error: unknown): SessionFailureDto {
  if (
    typeof error === "object" &&
    error !== null &&
    "summary" in error &&
    typeof (error as { summary: unknown }).summary === "string"
  ) {
    return error as SessionFailureDto;
  }
  return { summary: String(error), detail: null, remediation: null };
}
