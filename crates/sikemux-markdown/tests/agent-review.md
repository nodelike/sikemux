# Review of the ssh reconnect change

Overall this is close. Three things before it merges:

1. **Quoting.** `run_reconnect` builds the command with `format!`, so a host alias with a space breaks it:
   ```rust
   let command = format!("ssh {} -- {}", host, script);
   ```
   Pass the arguments separately instead.
2. **Backoff.** The loop in src-tauri/src/ssh.rs:88 retries every 500 ms forever.
   - cap it at 30 s
   - reset the delay after a successful connect

     > the old code did this in `on_connected`, see ssh.rs#L140
3. **Tests.** Nothing covers a dropped connection. Something like:

   | Case | Expected | Covered |
   | :-- | :-- | --- |
   | host down | retry with backoff | no |
   | auth fails | stop, show `Permission denied` | yes |
   | `a \| b` in alias | quoted | no |

---

Smaller notes:

* `SshError::Io` swallows the exit code. <!-- mention in the PR -->
* The doc comment on `Session::spawn` says *always*, but it returns early on Windows.
* ~~Rename `retry2`~~ done in 4f1c2ab.

<details>
<summary>Full log</summary>

```
ssh: connect to host dev port 22: Connection refused
retrying in 500ms
```
</details>

Links worth reading: https://man.openbsd.org/ssh_config.5#ServerAliveInterval, (see https://en.wikipedia.org/wiki/Exponential_backoff) and the RFC at <https://www.rfc-editor.org/rfc/rfc4254>.

Final check list:
- [x] builds on macOS
- [ ] builds on Linux
  - [ ] with `--no-default-features`
