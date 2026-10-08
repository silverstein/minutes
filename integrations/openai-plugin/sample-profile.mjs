import { cp, mkdir, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

// Keep sample retrieval off the user's library and configuration. These are
// the same public synthetic records used by the direct MCP qualification.
export async function prepareSampleProfile(root, fixtures) {
  const profile = path.join(root, 'sample-profile');
  const meetings = path.join(profile, 'meetings');
  const config = path.join(profile, 'config');
  const minutesHome = path.join(profile, 'minutes-home');
  const cache = path.join(profile, 'cache');
  const qmdConfig = path.join(config, 'qmd');
  const qmdCache = path.join(cache, 'qmd');
  const configPath = path.join(config, 'minutes', 'config.toml');
  await mkdir(config, { recursive: true, mode: 0o700 });
  await mkdir(minutesHome, { recursive: true, mode: 0o700 });
  await mkdir(qmdConfig, { recursive: true, mode: 0o700 });
  await mkdir(qmdCache, { recursive: true, mode: 0o700 });
  await mkdir(path.dirname(configPath), { recursive: true, mode: 0o700 });
  await cp(fixtures, meetings, { recursive: true, dereference: false });
  const records = (await readdir(meetings, { withFileTypes: true }))
    .filter(entry => entry.isFile() && entry.name.endsWith('.md'));
  if (records.length !== 5) throw new Error('Sample installation requires the five canonical synthetic meetings.');
  await writeFile(configPath, `output_dir = ${JSON.stringify(meetings)}\n`, { mode: 0o600 });
  // The MCP/correction layer uses MINUTES_HOME, while the native engine's
  // history, recovery and QMD retirement state use MINUTES_DATA_DIR.
  // Isolate both; overriding only one still consults the user's .minutes.
  // An installed QMD may inspect its SQLite cache during retirement even
  // when its config is empty. Override its explicit selectors as well, so
  // ambient vendor/config overrides cannot reach the user's registry.
  return { MINUTES_HOME: minutesHome, MINUTES_DATA_DIR: minutesHome,
    MINUTES_CONFIG_PATH: configPath, XDG_CONFIG_HOME: config,
    XDG_CACHE_HOME: cache, QMD_CONFIG_DIR: qmdConfig,
    INDEX_PATH: path.join(qmdCache, 'index.sqlite'), MEETINGS_DIR: meetings };
}
