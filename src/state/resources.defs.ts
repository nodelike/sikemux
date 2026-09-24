import { agentApi, type AgentInfo, type AgentModelInfo, type AgentSession, type AgentUsage } from "../api/agents";
import type { AgentRuntimeProfile } from "../agentProfiles";
import { filesApi } from "../api/files";
import { git, type DiscoveredRepo, type GitOverview, type GitRemote, type GitRemoteBranch, type GitStash } from "../api/git";
import { settingsApi, type ProjectEntry } from "../api/settings";
import { sshApi, type SshHost } from "../api/ssh";
import type { AgentType, ProjectRoot } from "./types";
import { resource } from "./resources";

export const gitOverviewR = resource({
    kind: "git.overview",
    fetch: (repo: string): Promise<GitOverview> => git.overview(repo),
    staleAfterMs: 5_000,
});

export const gitDiscoveredReposR = resource({
    kind: "git.discoveredRepos",
    fetch: (root: string): Promise<DiscoveredRepo[]> => git.discoverRepos(root),
    staleAfterMs: 5_000,
});

export const gitRemotesR = resource({
    kind: "git.remotes",
    fetch: (repo: string): Promise<GitRemote[]> => git.remotes(repo),
    staleAfterMs: 5 * 60_000,
});

export const gitRemoteBranchesR = resource({
    kind: "git.remoteBranches",
    fetch: (repo: string, remote: string): Promise<GitRemoteBranch[]> => git.remoteBranches(repo, remote),
    staleAfterMs: 30_000,
});

export const gitStashesR = resource({
    kind: "git.stashes",
    fetch: (repo: string): Promise<GitStash[]> => git.stashList(repo),
    staleAfterMs: 30_000,
});

export const agentCatalogR = resource({
    kind: "agents.catalog",
    fetch: (profiles: AgentRuntimeProfile[]): Promise<AgentInfo[]> => agentApi.available(profiles),
    staleAfterMs: 60_000,
});

export const agentModelsR = resource({
    kind: "agents.models",
    fetch: (type: AgentType, executablePath?: string, configPath?: string): Promise<AgentModelInfo[]> =>
        agentApi.models(type, executablePath, configPath),
    staleAfterMs: 5 * 60_000,
});

export const agentUsageR = resource({
    kind: "agents.usage",
    fetch: (type: AgentType, executablePath?: string, configPath?: string): Promise<AgentUsage> => agentApi.usage(type, executablePath, configPath),
    staleAfterMs: 5 * 60_000,
});

export const agentSessionsR = resource({
    kind: "agents.sessions",
    fetch: (type: AgentType, cwd: string, configPath?: string): Promise<AgentSession[]> => agentApi.sessions(type, cwd, configPath),
    staleAfterMs: 0,
});

export const filesListR = resource({
    kind: "files.list",
    fetch: (repo: string): Promise<string[]> => filesApi.list(repo),
    staleAfterMs: 60_000,
});

export const projectRootsScanR = resource({
    kind: "settings.projectRootsScan",
    fetch: (roots: ProjectRoot[]): Promise<ProjectEntry[]> => settingsApi.scanProjectRoots(roots),
    staleAfterMs: 60_000,
});

export const sshHostsR = resource({
    kind: "ssh.hosts",
    fetch: (): Promise<SshHost[]> => sshApi.hosts(),
    staleAfterMs: 5 * 60_000,
});
