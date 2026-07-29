# Security Policy

## Supported Versions

This project aims for strict [SemVer](https://semver.org/) compliance. This
means that upgrading to a new minor or patch version should be trivial, but
upgrading to a new major version may incur some significant costs on your side.

With that in mind, if we denote major.minor.patch the project's version
number, the policy for security updates support is that...

- The latest major.minor.patch version published on crates.io is supported
  (obviously).
- Each previously published major release remains supported, for users of the
  latest minor.patch version, during 6 months after the release of the next
  major release.


## Reporting a Vulnerability

### Basics

Please do not report security vulnerabilities (any bug that has security
implications if e.g. a setuid binary happens to use `triple-buffer`) using
public channels such as...

- A project's issue tracker
- Social media
- A [CVE
  Report](https://en.wikipedia.org/wiki/Common_Vulnerabilities_and_Exposures)

Doing so is the computer security equivalent of reporting a fire safety risk by
starting a fire: you are informing attackers that software has a problem before
a fix is available, and they will be able to exploit this information for the
entire duration it will take us to come up with a fix, which is [Very
Bad](https://en.wikipedia.org/wiki/Zero-day_(computing)).

Instead, you should privately report the vulnerability to the project, give it
some time to come up with a fix, and only report the vulnerability publicly once
a patch release is available for all supported major versions (except in
worst-case scenarios discussed below).

### Reporting guidelines

Please privately submit us a security advisory using [this dedicated
form](https://github.com/HadrienG2/triple-buffer/security/advisories/new). A
good security advisory should...

- Describe the vulnerability
- Assess its impact on downstream users of the library
- Provide instructions on how to replicate the issue locally
- Additionally provide a patch that addresses the issue, if you have one
  available (otherwise the project will take care of writing it)

### Worst-case scenarios

While private reporting and embargo until a bugfix is available is the preferred
course of action on our side, we are well aware that if we spend too much time
without fixing the vulnerability, or if you become aware that another attacker
has found out about the vulnerability and started exploiting it, you may want to
publicly report the vulnerability anyway.

Here's what you can expect from us in terms of response time :

- At the time of writing, the project only has one permanent maintainer,
  [**@HadrienG2**](https://github.com/HadrienG2). I will aim for fast replies
  (less than a week, ideally next-day), but I may be momentarily unavailable
  for various reasons: holidays, sickness... So please ping me a few times
  (e.g. after a week and a month) before concluding that I am unresponsive and
  must have abandoned the project.
- Unless the vulnerability is extremely complex and requires a major library
  rewrite to fix, 90 days should be more than enough time for me to fix it.
