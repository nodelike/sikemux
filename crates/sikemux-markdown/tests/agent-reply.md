I looked at the failing test and traced it to `src/chat/reducer.ts:142`. The reducer drops a chunk when two updates land in the same frame.

## What changed

1. **Reducer** — `applyChunk` now appends instead of replacing:
   - it keys pending text by `messageId`
   - it flushes on `turn_end`, see [the ACP spec](https://agentclientprotocol.com/protocol/prompt-turn)
2. **Tests** — added a case in src/chat/reducer.test.ts that sends two chunks in one frame.
3. Cleaned up ~~dead~~ unused helpers.

| File | Lines | Status |
| --- | ---: | :---: |
| `src/chat/reducer.ts` | +18 −6 | done |
| src/chat/reducer.test.ts | +41 | done |
| README.md | +2 | *pending* |

```ts
export function applyChunk(state: ChatState, chunk: Chunk): ChatState {
    const pending = state.pending.get(chunk.messageId) ?? "";
    state.pending.set(chunk.messageId, pending + chunk.text);
    return state;
}
```

```diff
--- a/src/chat/reducer.ts
+++ b/src/chat/reducer.ts
@@ -140,3 +140,3 @@
-    state.pending.set(chunk.messageId, chunk.text);
+    state.pending.set(chunk.messageId, pending + chunk.text);
```

> **Note:** the fix does not touch `useXterm.ts`; that flake is a separate issue tracked at https://github.com/nodelike/sikemux/issues/412.

- [x] reproduce the drop
- [x] fix the reducer
- [ ] run the full suite on CI

Next I can run `pnpm test src/chat` or open www.example.com for the docs. Ping me at dev@example.com if anything looks off.
