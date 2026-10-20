---
name: databases
description: Working with databases such as PostgreSQL, MySQL or MariaDB, SQLite, Redis and MongoDB. Dumps, restores, upkeep and health checks without breaking a live database.
---
# Databases

## Find out first
- Which database and version: look for running services and tools (`pg_dump --version`, `mysqldump --version`, `sqlite3 --version`, `redis-cli --version`, `mongodump --version`).
- How to connect: host, port, user, database names. Passwords come from vault values or the tool's own credentials file (`~/.pgpass`, `~/.my.cnf`), never from the step's text.
- How big it is, and when it is busy.

## Dumps
- PostgreSQL: `pg_dump -Fc` for each database, and `pg_dumpall --globals-only` for roles.
- MySQL or MariaDB: `mysqldump --single-transaction --routines --triggers`.
- SQLite: `sqlite3 <db> ".backup '<copy>'"`, never a plain copy of a file in use.
- Redis: `redis-cli BGSAVE`, wait until `LASTSAVE` changes, then copy the dump file.
- MongoDB: `mongodump --archive=<file> --gzip`.
- Write to a temp name and rename when done. Check the exit code and that the file isn't empty.
- Find the databases to dump each run (list them from the server), unless the user named them.

## Restores and changes
- Never drop, empty or restore over a database unless the user asked for that one by name. Restore into a new database first and compare.
- Before a schema change or a big delete, dump first in a step before it, and add an ask step.

## Upkeep
- Use the database's own commands (VACUUM and ANALYZE, OPTIMIZE TABLE, compaction), at quiet hours, one database at a time.

## Health
- Connect and run a tiny query (`SELECT 1`), check replication lag and the free disk where the data lives, and report the numbers.
