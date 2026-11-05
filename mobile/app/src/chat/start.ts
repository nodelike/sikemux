import { File } from 'expo-file-system';
import type { ConnectionLike } from '@sikemux/native';

import { problem as problemOf } from '@/core/problem';
import { callHost } from '@/core/hostCalls';
import { chatSession } from './useChat';
import { uploadEach, type Attachment } from './uploads';

/** A new chat's first message, and how far an earlier try at sending it got. */
type FirstMessage = {
  core: string;
  launcherId: string;
  projectId: string;
  mode: string | undefined;
  text: string;
  attachments: Attachment[];
  worktree: boolean;
  /** The agent an earlier try started, sent to again rather than starting another. */
  agentId?: string;
  /** An earlier try made the chat's worktree, so the agent starts there rather than in another. */
  inWorktree: boolean;
};

/** `failed` hands the files back as they ended up, and says whether the host made the worktree for the next try. */
type StartOutcome =
  { kind: 'started'; agentId: string } | { kind: 'failed'; problem: string; files: Attachment[]; inWorktree: boolean };

type Progress = {
  /** The agent is running on the host, before its first message has gone. */
  started: (agentId: string) => void;
  /** Names each wait the person sees. */
  step: (label: string) => void;
};

/**
 * Starts the agent, makes its worktree when asked, puts the files on the host and leaves the message with the chat
 * to go once it is attached.
 */
export async function startChat(connection: ConnectionLike, first: FirstMessage, progress: Progress): Promise<StartOutcome> {
  let { inWorktree } = first;
  const failed = (problem: string, files = first.attachments): StartOutcome => ({ kind: 'failed', problem, files, inWorktree });
  try {
    let agentId = first.agentId;
    if (!agentId) {
      agentId = await connection.startChat(first.launcherId, first.projectId, first.mode);
      progress.started(agentId);
    }
    if (first.worktree && !inWorktree) {
      progress.step('Creating worktree');
      const made = await callHost(connection, 'createAgentWorktree', { agentId, message: first.text });
      inWorktree = true;
      if (made.setupError) return failed(`${made.setupError}. The worktree is kept at ${made.worktree?.path}; send again to start there.`);
      progress.step('Starting the agent in the worktree');
    }
    const { sent, files } = await upload(connection, agentId, first.attachments);
    if (!sent) return failed('Could not send every file to the host', files);
    chatSession(first.core, agentId).sendWhenAttached(first.text, sent);
    return { kind: 'started', agentId };
  } catch (error) {
    return failed(problemOf(error));
  }
}

/** Answers the files as they ended up, and all of them sent or null when one did not get there. */
async function upload(connection: ConnectionLike, agentId: string, picked: Attachment[]) {
  let files = picked;
  const sent = await uploadEach(
    picked,
    async (attachment) => connection.attachFile(agentId, attachment.name, attachment.mime, await new File(attachment.uri).arrayBuffer()),
    (id, change) => {
      files = files.map((known) => (known.id === id ? { ...known, ...change } : known));
    },
    problemOf,
  );
  return { sent, files };
}
