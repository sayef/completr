# Security policy

## Supported versions

strato is pre-1.0. Security fixes go into the latest release.

## Reporting a vulnerability

Please do not open a public issue. Report vulnerabilities privately through
[GitHub security advisories](https://github.com/sayef/strato/security/advisories/new), or by email
to [msi.sayef@gmail.com](mailto:msi.sayef@gmail.com).

Include what you found, how to reproduce it and what you think the impact is. We will acknowledge the
report within five working days and keep you informed until it is resolved. We credit reporters in the
advisory unless you prefer otherwise.

## Scope

strato reads segment files and manifests from storage you configure. Segment files carry a checksum and
are validated on load, but strato trusts storage that only authorised writers can modify. Reports showing
crashes, out-of-bounds reads or excessive resource use from files that pass validation are in scope.
