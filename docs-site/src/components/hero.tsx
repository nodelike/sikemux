import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { AgentMark } from "./agent-mark";

const INSTALL = "curl -fsSL https://sikemux.com/install | sh";

const AGENTS = [
  ["claude", "Claude Code", "claude-code"],
  ["codex", "Codex", "codex"],
  ["opencode", "OpenCode", "opencode"],
  ["hermes", "Hermes", "hermes"],
  ["pi", "Pi", "pi"],
  ["omp", "OMP", "omp"],
  ["grok", "Grok", "grok"],
  ["muse", "Muse", "muse"],
];

export function Hero({ title, description }: { title: string; description: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <header className="hero">
      <p className="hero-label">Documentation</p>
      <h1>{title}</h1>
      <p className="hero-lede">{description}</p>
      <div className="hero-actions">
        <a className="hero-btn hero-btn-primary" href="/getting-started/install/">
          Get started
        </a>
        <button
          type="button"
          className="hero-install"
          aria-label="Copy the install command"
          data-copied={copied || undefined}
          onClick={() => {
            void navigator.clipboard.writeText(INSTALL);
            setCopied(true);
            setTimeout(() => setCopied(false), 1600);
          }}
        >
          <span className="hero-prompt" aria-hidden="true">
            $
          </span>
          <code>{INSTALL}</code>
          {copied ? <Check size={14} /> : <Copy size={14} />}
        </button>
      </div>
      <nav className="hero-agents" aria-label="Agents">
        {AGENTS.map(([agent, name, slug]) => (
          <a key={agent} href={`/each-agent/${slug}/`}>
            <AgentMark agent={agent} />
            {name}
          </a>
        ))}
      </nav>
    </header>
  );
}
