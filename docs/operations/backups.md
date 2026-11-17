# Database backups

Every night at 03:30 India time (plus up to 20 minutes), `sikemux-backup.timer` on citadel
dumps the `sikemux` database, encrypts it with [age](https://age-encryption.org) and uploads
it to the Cloudflare R2 bucket `sikemux-backups`. Backups are kept 30 days, as the privacy
policy promises. The same run copies the stash's files (see [The stash](#the-stash)). Phone
update assets are not backed up: they are rebuilt from the git tags.

Citadel holds only the age public key. It can write a backup but never read one: the private
key stays offline with the owner, and the restore drill takes it on stdin, never from disk.

## What runs

| Piece               | On citadel                                       | From                             |
| ------------------- | ------------------------------------------------ | -------------------------------- |
| Backup script       | `/usr/local/lib/sikemux/backup-database`         | `server/deploy/backup-database`  |
| Restore script      | `/usr/local/lib/sikemux/restore-database`        | `server/deploy/restore-database` |
| Stash restore       | `/usr/local/lib/sikemux/restore-stash`           | `server/deploy/restore-stash`    |
| R2 setup both share | `/usr/local/lib/sikemux/lib.sh`                  | `server/deploy/lib.sh`           |
| Units               | `sikemux-backup.service`, `sikemux-backup.timer` | `server/deploy/`                 |
| R2 credentials      | `/etc/sikemux/backup.env` (root, 600)            | written by hand                  |
| age public keys     | `/etc/sikemux/backup-recipients.txt` (root, 644) | written by hand                  |

`setup-citadel.sh` installs all of it, creates the OS user `sikemux-backup` and the database
role `sikemux_backup`, which can read everything (`pg_read_all_data`) and write nothing, and
lets only that user log in as that role, through the local socket.

`/etc/sikemux/backup.env`:

```sh
R2_ACCOUNT_ID=<Cloudflare account ID>
R2_ACCESS_KEY_ID=<R2 token scoped to sikemux-backups: Object Read & Write>
R2_SECRET_ACCESS_KEY=<its secret>
R2_BUCKET=sikemux-backups
# R2_ENDPOINT=https://<account ID>.eu.r2.cloudflarestorage.com   only for a bucket with a jurisdiction
```

Objects are named `sikemux/YYYY/MM/DD/sikemux-<UTC time>.dump.age`: a `pg_dump` custom-format
archive, encrypted. After each upload the script checks the stored size and MD5 match, then
deletes backups older than 30 days. Any failure fails the unit; read it with
`journalctl -u sikemux-backup`. `sikemux-watch.timer` posts to Discord's #alerts when the last
run failed or started more than 26 hours ago, and the monthly drill also fails if the newest
backup is over two days old.

R2 deletes old backups on its own too, in case the script stops running. The bucket has a
lifecycle rule `expire-backups`: prefix `sikemux/`, delete objects 30 days after upload, and
abort incomplete multipart uploads after 1 day.

## The key

The owner generates the key on their own computer, never on citadel:

```sh
age-keygen -o sikemux-backup.key        # prints the public key, age1...
security add-generic-password -a sikemux -s "Sikemux backup key" \
  -w "$(grep AGE-SECRET-KEY sikemux-backup.key)"
```

Keep a second copy of the `AGE-SECRET-KEY-1...` line in the private records repository, then
delete `sikemux-backup.key`. Losing every copy makes every backup unreadable. Put the public
key on its own line in `/etc/sikemux/backup-recipients.txt`; the file can list more than one
key, and each listed key can open new backups.

To change keys, add the new public key, wait 30 days for every backup to be encrypted to it,
then remove the old one.

## Monthly restore drill

On the first of each month, restore the newest backup into a scratch database and compare it
with the live one:

```sh
security find-generic-password -a sikemux -s "Sikemux backup key" -w   # copy the key
ssh citadel
sudo /usr/local/lib/sikemux/restore-database   # paste the key, then Ctrl-D
```

It downloads the backup, decrypts it in memory, restores it into `sikemux_restore_check`,
prints each table's row count beside the live count and the latest migration of both, and
drops the scratch database, even when it fails. It fails if the archive does not restore, if
the backup has no rows while the live database has some, if both are on the same migration but
have different tables, or if the newest backup is over two days old. Counts differ a little,
since the live database has moved on since the backup.

`--object sikemux/YYYY/MM/DD/sikemux-<time>.dump.age` drills an older backup.

## Recovering

Restore into a new database, keep it, then swap it in:

```sh
sudo /usr/local/lib/sikemux/restore-database --database sikemux_recovered --keep
sudo systemctl stop sikemux-api sikemux-purge.timer
sudo psql -U nodelike -d postgres <<'SQL'
alter database sikemux rename to sikemux_broken;
alter database sikemux_recovered rename to sikemux;
revoke all on database sikemux from public;
grant connect on database sikemux to sikemux_backup;
SQL
sudo systemctl start sikemux-api sikemux-purge.timer
```

The restored tables keep their owner, `sikemux`, so the API and its migrations work as before.
Drop `sikemux_broken` once the API is healthy.

## The stash

Stashed files live in `/var/lib/sikemux-stash`, each already encrypted with the stash key in
`/etc/sikemux/stash.key`. The nightly run copies new files to `stash/files` in the bucket as they
are; a file deleted on citadel moves to `stash/deleted/<date>` and is deleted there after 30
days. The stash key goes up as `stash/key.age`, sealed to the same age keys as the database, so
the files can be read back without citadel. The `expire-backups` rule covers only `sikemux/`, so
it never touches `stash/files`.

The monthly drill checks the stash too:

```sh
sudo /usr/local/lib/sikemux/restore-stash   # paste the same key, then Ctrl-D
```

It opens `stash/key.age` and fails unless it matches citadel's key, copies every file into a
scratch folder, compares the count with the live folder and with the items that have a file,
decrypts 20 of them at random with the API's `check-stash`, and deletes the folder.

To recover on a new citadel, install `age` and `rclone`, copy `server/deploy` and `backup.env`
there, then run the script from that copy before `setup-citadel.sh`, so setup keeps the restored
key instead of making one and hands the files to the API:

```sh
sudo BACKUP_ENV=./backup.env ./restore-stash --dir /var/lib/sikemux-stash --keep
```
