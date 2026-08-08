import { mkdtemp, readFile, stat, writeFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import { createHash } from "crypto";
import { describe, expect, it } from "vitest";

import {
  downloadReleaseBinaryWithChecksum,
  extractZipWithPowerShell,
  findSha256ForAsset,
  parseSha256Sums,
} from "./autoInstall.js";

function sha256(input: string): string {
  return createHash("sha256").update(input).digest("hex");
}

describe("parseSha256Sums", () => {
  it("parses standard sha256sum output", () => {
    const mac = "a".repeat(64);
    const linux = "b".repeat(64);

    expect(
      parseSha256Sums(`
${mac}  minutes-macos-arm64
${linux} *minutes-linux-x64
`)
    ).toEqual([
      { filename: "minutes-macos-arm64", sha256: mac },
      { filename: "minutes-linux-x64", sha256: linux },
    ]);
  });

  it("ignores blank, comment, and malformed lines", () => {
    const windows = "C".repeat(64);
    expect(
      parseSha256Sums(`
# release checksums
not a checksum

${windows}  minutes-windows-x64.exe
`)
    ).toEqual([{ filename: "minutes-windows-x64.exe", sha256: windows.toLowerCase() }]);
  });

  it("finds entries by basename for nested artifact paths", () => {
    const checksum = "d".repeat(64);
    expect(
      findSha256ForAsset(
        `${checksum}  dist/minutes-linux-x64\n`,
        "minutes-linux-x64"
      )
    ).toBe(checksum);
  });
});

describe("downloadReleaseBinaryWithChecksum", () => {
  it("downloads the sums first, verifies the binary, and installs it", async () => {
    const dir = await mkdtemp(join(tmpdir(), "minutes-mcp-install-"));
    const targetPath = join(dir, "minutes");
    const payload = "verified cli";
    const checksum = sha256(payload);
    const calls: string[] = [];

    const execFileAsync = async (_file: string, args: readonly string[]) => {
      const outputPath = args[2] as string;
      const url = args[3] as string;
      calls.push(url);
      if (url.endsWith("/SHA256SUMS.txt")) {
        await writeFile(outputPath, `${checksum}  minutes-linux-x64\n`);
      } else if (url.endsWith("/minutes-linux-x64")) {
        await writeFile(outputPath, payload);
      }
    };

    await downloadReleaseBinaryWithChecksum({
      binaryName: "minutes-linux-x64",
      targetPath,
      execFileAsync,
      baseUrl: "https://example.test/download",
    });

    expect(calls).toEqual([
      "https://example.test/download/SHA256SUMS.txt",
      "https://example.test/download/minutes-linux-x64",
    ]);
    await expect(readFile(targetPath, "utf8")).resolves.toBe(payload);
  });

  it("aborts and leaves no target binary when checksum verification fails", async () => {
    const dir = await mkdtemp(join(tmpdir(), "minutes-mcp-install-"));
    const targetPath = join(dir, "minutes");

    const execFileAsync = async (_file: string, args: readonly string[]) => {
      const outputPath = args[2] as string;
      const url = args[3] as string;
      if (url.endsWith("/SHA256SUMS.txt")) {
        await writeFile(outputPath, `${"0".repeat(64)}  minutes-linux-x64\n`);
      } else if (url.endsWith("/minutes-linux-x64")) {
        await writeFile(outputPath, "bad payload");
      }
    };

    await expect(
      downloadReleaseBinaryWithChecksum({
        binaryName: "minutes-linux-x64",
        targetPath,
        execFileAsync,
        baseUrl: "https://example.test/download",
      })
    ).rejects.toThrow("checksum mismatch");
    await expect(stat(targetPath)).rejects.toMatchObject({ code: "ENOENT" });
  });
});

describe("extractZipWithPowerShell", () => {
  function capture() {
    const calls: Array<{
      file: string;
      args: readonly string[];
      options?: { timeout?: number; env?: NodeJS.ProcessEnv };
    }> = [];
    const execFileAsync = async (
      file: string,
      args: readonly string[],
      options?: { timeout?: number; env?: NodeJS.ProcessEnv }
    ) => {
      calls.push({ file, args, options });
      return undefined;
    };
    return { calls, execFileAsync };
  }

  it("passes both paths through the environment, not the command text", async () => {
    const { calls, execFileAsync } = capture();
    await extractZipWithPowerShell({
      archivePath: "C:\\Users\\qa\\.minutes\\bin\\minutes-windows-x64.zip",
      destDir: "C:\\Users\\qa\\.minutes\\bin",
      execFileAsync,
    });

    expect(calls).toHaveLength(1);
    const [call] = calls;
    expect(call.file).toBe("powershell");
    expect(call.options?.env?.MINUTES_ZIP_PATH).toBe(
      "C:\\Users\\qa\\.minutes\\bin\\minutes-windows-x64.zip"
    );
    expect(call.options?.env?.MINUTES_ZIP_DEST).toBe("C:\\Users\\qa\\.minutes\\bin");
  });

  it("never reads $args, which -Command leaves empty", async () => {
    const { calls, execFileAsync } = capture();
    await extractZipWithPowerShell({
      archivePath: "C:\\tmp\\a.zip",
      destDir: "C:\\tmp\\out",
      execFileAsync,
    });

    const { args } = calls[0];
    const commandIndex = args.indexOf("-Command");
    expect(commandIndex).toBeGreaterThanOrEqual(0);
    const script = args[commandIndex + 1];

    // `powershell -Command` appends trailing arguments to the command text and
    // leaves $args empty; only -File binds them. Reading $args[0] made every
    // Windows install fail with "argument is null or empty".
    expect(script).not.toContain("$args");
    expect(script).toContain("$env:MINUTES_ZIP_PATH");
    expect(script).toContain("$env:MINUTES_ZIP_DEST");

    // Nothing may follow the script, or it lands in the command text.
    expect(args).toHaveLength(commandIndex + 2);
  });

  it("keeps an apostrophe home directory out of the command text", async () => {
    const { calls, execFileAsync } = capture();
    const home = "C:\\Users\\O'Brien\\.minutes\\bin";
    await extractZipWithPowerShell({
      archivePath: `${home}\\minutes-windows-x64.zip`,
      destDir: home,
      execFileAsync,
    });

    const { args, options } = calls[0];
    const script = args[args.indexOf("-Command") + 1];
    expect(script).not.toContain("O'Brien");
    expect(options?.env?.MINUTES_ZIP_DEST).toBe(home);
  });
});
