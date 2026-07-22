import { ESLint } from 'eslint';
import { readFile } from 'node:fs/promises';
import globals from 'globals';

const root = new URL('../../', import.meta.url);
const indexPath = new URL('tauri/src/index.html', root);
const source = await readFile(indexPath, 'utf8');
const scripts = [...source.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/gi)]
  .map((match) => match[1])
  .filter((script) => script.trim());
const inlineSource = scripts.join('\n;\n');

const eslint = new ESLint({
  overrideConfigFile: true,
  overrideConfig: [
    {
      languageOptions: {
        ecmaVersion: 'latest',
        sourceType: 'script',
        globals: {
          ...globals.browser,
          FitAddon: 'readonly',
          Terminal: 'readonly',
          marked: 'readonly',
        },
      },
      rules: {
        'no-undef': 'error',
      },
    },
  ],
});

const [result] = await eslint.lintText(inlineSource, { filePath: 'tauri/src/index.inline.js' });
const errors = result.messages.filter((message) => message.severity === 2);
if (errors.length > 0) {
  for (const error of errors) {
    process.stderr.write(`${error.ruleId || 'parse'}:${error.line}:${error.column} ${error.message}\n`);
  }
  process.exitCode = 1;
} else {
  process.stdout.write('Desktop frontend globals are bound.\n');
}
