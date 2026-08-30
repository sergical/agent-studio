// ============================================================================
// Skill Studio - skill-pack-name
// Mirrors the backend's `skill_pack::validate_pack_name` so the inline
// pack-name prompt can reject a bad name before it ever reaches an IPC call.
// ============================================================================

const PACK_NAME_PATTERN = /^[a-z0-9][a-z0-9-]*$/;
const MAX_PACK_NAME_LENGTH = 64;

/** Default suggestion shown in the pack-name prompt. */
export const DEFAULT_PACK_NAME = "my-skills";

/**
 * Returns an error message if `name` isn't a valid pack name, `undefined`
 * otherwise. Matches the backend's `^[a-z0-9][a-z0-9-]{0,63}$` rule exactly.
 */
export function validatePackName(name: string): string | undefined {
  if (name.length === 0) return "Pack name can't be empty.";
  if (name.length > MAX_PACK_NAME_LENGTH) {
    return `Pack name must be ${MAX_PACK_NAME_LENGTH} characters or fewer.`;
  }
  if (!PACK_NAME_PATTERN.test(name)) {
    return "Pack name must start with a lowercase letter or digit, and can only use lowercase letters, digits, and dashes.";
  }
  return undefined;
}
