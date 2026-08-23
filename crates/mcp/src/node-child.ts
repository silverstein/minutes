/**
 * Environment for a helper that must run as Node through process.execPath.
 *
 * Electron hosts such as Claude Desktop expose their app executable as
 * process.execPath. Spawning that executable with a JavaScript file does not
 * start Node unless ELECTRON_RUN_AS_NODE is set for the child. Keep the switch
 * scoped to helpers whose argv is a Node program; ordinary Minutes CLI children
 * must continue to receive their existing policy environment unchanged.
 */
export function nodeChildEnvironment(
  base: NodeJS.ProcessEnv = process.env,
  electronVersion: string | undefined = process.versions.electron
): NodeJS.ProcessEnv {
  const environment = { ...base };
  if (electronVersion) {
    environment.ELECTRON_RUN_AS_NODE = "1";
  }
  return environment;
}
