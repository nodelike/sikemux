import type { InboxItem, ProjectInfo, Snapshot } from '@/core/protocol';

export function folder(path: string): string {
  return path.split('/').filter(Boolean).pop() ?? path;
}

/** `word` as it reads beside `count`: "1 agent", "2 agents". */
export function counted(word: string, count: number): string {
  return count === 1 ? word : `${word}s`;
}

/** The host's project with this id or folder. */
export function projectAt(snapshot: Snapshot, where: string): ProjectInfo | undefined {
  return snapshot.workspace.projects.find((project) => project.id === where || project.path === where);
}

/** The name of the project with this id or folder, else the last folder of `path`. */
export function projectName(snapshot: Snapshot, where: string, path = where): string {
  return projectAt(snapshot, where)?.name ?? folder(path);
}

export function inProject(project: ProjectInfo, path: string): boolean {
  return path === project.path || path.startsWith(`${project.path}/`);
}

/** The id of the project the host keeps the agent in, which its worktree's folder does not show. */
export function projectOf(snapshot: Snapshot, agentId: string): string | undefined {
  return snapshot.workspace.agents?.find((agent) => agent.id === agentId)?.project;
}

export function agentIn(snapshot: Snapshot, project: ProjectInfo, agentId: string, cwd: string): boolean {
  const kept = projectOf(snapshot, agentId);
  return kept ? kept === project.id : inProject(project, cwd);
}

/** The item's card is on `project`'s board, or its agent works there. */
export function itemIn(snapshot: Snapshot, item: InboxItem, project: ProjectInfo): boolean {
  if (!item.card && item.agentId && item.project !== undefined) return agentIn(snapshot, project, item.agentId, item.project);
  const where = item.card?.project ?? item.project;
  return where !== undefined && (where === project.id || inProject(project, where));
}
