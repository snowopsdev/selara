// The expected team comes from release credentials, never from the artifact.
export function developerIdRequirement(teamId) {
  if (typeof teamId !== "string" || !/^[A-Z0-9]{10}$/.test(teamId)) {
    throw new Error("A valid APPLE_TEAM_ID is required to verify recovered release code");
  }
  // codesign treats a bare string as a filename; '=' selects inline source.
  return `=anchor apple generic and certificate leaf[subject.OU] = "${teamId}" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists`;
}
