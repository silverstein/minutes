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
      className="group relative bg-[#0a0a0a] border border-white/[0.06] rounded-[2px] px-5 py-2.5 font-mono text-[13px] text-[#ededed] cursor-pointer transition-all hover:border-white/[0.12] hover:bg-[#111]"
    >
      <span
        className={`block font-sans text-[11px] uppercase tracking-wider ${
          compact ? "text-[#ededed]" : "mb-1 text-[#666]"
        }`}
      >
        {label}
      </span>
      {!compact && cmd}
      {copied && (
        <span className="absolute inset-0 flex items-center justify-center bg-[#0a0a0a] rounded-[2px] text-[#00cc88] font-sans text-xs">
          Copied!
        </span>
      )}
    </button>
  );
}
