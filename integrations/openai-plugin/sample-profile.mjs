import { cp, mkdir, readdir } from 'node:fs/promises';
import path from 'node:path';

// Keep sample retrieval off the user's library and configuration. These are
// the same public synthetic records used by the direct MCP qualification.
export async function prepareSampleProfile(root, fixtures) {
  const profile = path.join(root, 'sample-profile');
  const meetings = path.join(profile, 'meetings');
  const config = path.join(profile, 'config');
  const minutesHome = path.join(profile, 'minutes-home');
  await mkdir(config, { recursive: true, mode: 0o700 });
  await mkdir(minutesHome, { recursive: true, mode: 0o700 });
  await cp(fixtures, meetings, { recursive: true, dereference: false });
  const records = (await readdir(meetings, { withFileTypes: true }))
    .filter(entry => entry.isFile() && entry.name.endsWith('.md'));
  if (records.length !== 5) throw new Error('Sample installation requires the five canonical synthetic meetings.');
  // The MCP/correction layer uses MINUTES_HOME, while the native engine's
  // history, recovery and QMD retirement state use MINUTES_DATA_DIR.
  // Isolate both; overriding only one still consults the user's .minutes.
  return { MINUTES_HOME: minutesHome, MINUTES_DATA_DIR: minutesHome,
    XDG_CONFIG_HOME: config, MEETINGS_DIR: meetings };
}
