export function PublicFooter() {
  return (
    <footer className="mt-16 border-t border-[color:var(--border)] py-10 text-center text-[13px] text-[var(--text-secondary)]">
      <p>minutes is MIT licensed and free forever.</p>
      <p className="mt-3 flex flex-wrap items-center justify-center gap-x-3 gap-y-2">
        <a href="/" className="hover:text-[var(--accent)]">
          Home
        </a>
        <span aria-hidden="true">·</span>
        <a href="/docs" className="hover:text-[var(--accent)]">
          Docs
        </a>
        <span aria-hidden="true">·</span>
        <a href="/for-agents" className="hover:text-[var(--accent)]">
          For agents
        </a>
        <span aria-hidden="true">·</span>
        <a href="/proof" className="hover:text-[var(--accent)]">
          Proof
        </a>
        <span aria-hidden="true">·</span>
        <a href="/compare" className="hover:text-[var(--accent)]">
          Compare
        </a>
        <span aria-hidden="true">·</span>
        <a href="/resources/best-meeting-tools-for-claude-code-and-codex" className="hover:text-[var(--accent)]">
          Resources
        </a>
      </p>
      <p className="mt-2 flex flex-wrap items-center justify-center gap-x-3 gap-y-2">
        <a href="/llms.txt" className="hover:text-[var(--accent)]">
          llms.txt
        </a>
        <span aria-hidden="true">·</span>
        <a href="https://github.com/silverstein/minutes" className="hover:text-[var(--accent)]">
          GitHub
        </a>
        <span aria-hidden="true">·</span>
        <a
          href="https://github.com/silverstein/minutes/discussions"
          className="hover:text-[var(--accent)]"
        >
          Discussions
        </a>
      </p>
    </footer>
  );
}
