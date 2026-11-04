# Security policy

TursoBench is an independent project, not affiliated with, aligned with, endorsed
by, or maintained by Turso or the Turso team. Report TursoBench-specific issues to
this repository's maintainer.

## Supported versions

Security fixes target the latest code on the default branch during the Linux
preview. Older snapshots and preview packages have no guaranteed backports.
There is no guaranteed response or fix timeline.

## Report a vulnerability

Do not post vulnerability details, exploit code, private databases or credentials
in public issues or pull requests.

1. Use GitHub's **Report a vulnerability** option on the repository's
   [Security page](https://github.com/pmfleming/tursobench/security), if available.
2. If private reporting is unavailable, use a private contact method listed on
   [the maintainer's profile](https://github.com/pmfleming), if available. Otherwise,
   open an issue requesting a private security contact, without disclosing details.
3. In the private report, include the affected commit/version, environment,
   impact and minimal reproduction using synthetic data. Suggest a fix if known.

Allow the maintainer to investigate and coordinate a fix before publishing details.
For issues isolated to an upstream dependency, use that upstream project's
security reporting process; mention the TursoBench impact in a private report here.

## Preview safety boundaries

- Open trusted, local database files. Read-only is the default; write access
  requires explicit opt-in. Linux OFD admission locks reject legacy writers;
  compatible Turso 0.8.3 SDK peers can use experimental multiprocess WAL. Other
  engines, lock-disabled clients and source/sidecar replacement remain unsupported.
  See [shared access](docs/shared-access.md) for the qualified protocol and limits.
- Back up closed databases with all required WAL state before writes. Copying
  only the main file of a live database is not a safe backup.
- Queries are not sandboxed. Discarding results does not cancel execution or
  roll back writes; engine memory use is not bounded by the result display limits.
- SQL history stores literals. Remove sensitive history and sanitize exports,
  screenshots and bug reports. Query parameters are not persisted.

See the [guide](docs/guide.md#important-limits) and
[release status](docs/release.md) for compatibility limits and unresolved gates.
