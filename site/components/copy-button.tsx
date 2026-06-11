"use client";

import { useState } from "react";

export function CopyButton({
  label,
  cmd,
  compact = false,
}: {
  label: string;
  cmd: string;
  compact?: boolean;
}) {
  const [copied, setCopied] = useState(false);

  return (
    <button
      onClick={() => {
        navigator.clipboard.writeText(cmd).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
      className="group relative cursor-pointer rounded-[5px] border border-[color:var(--border)] bg-[var(--bg-elevated)] px-5 py-2.5 font-mono text-[13px] text-[var(--text)] shadow-[var(--shadow-panel)] transition-all hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
    >
      <span
        className={`block font-sans text-[11px] uppercase tracking-wider ${
          compact ? "text-[var(--text)]" : "mb-1 text-[var(--text-secondary)]"
        }`}
      >
        {label}
      </span>
      {!compact && cmd}
      {copied && (
        <span className="absolute inset-0 flex items-center justify-center rounded-[5px] bg-[var(--bg-elevated)] font-sans text-xs text-[var(--accent)]">
          Copied!
        </span>
      )}
    </button>
  );
}
