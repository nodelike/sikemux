import type { ProjectInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';

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

/** The id of the project the host says the open agent is in; none for one outside every project. */
export function projectOf(snapshot: Snapshot, agentId: string): string | undefined {
  return snapshot.inbox.agents.find((agent) => agent.agentId === agentId)?.project;
}

/** How many things in `project` wait on the person, as the host counts them. */
export function needsIn(snapshot: Snapshot, project: ProjectInfo): number {
  return snapshot.inbox.needs[project.id] ?? 0;
}
