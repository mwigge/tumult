---
title: tumult-ssh
parent: Plugins
nav_order: 3
---

# tumult-ssh

Run a command on another host over SSH. A step runs on the machine that
runs Tumult, so this native plugin is how a step reaches another host.

```mermaid
flowchart LR
    accTitle: A step that runs a command over SSH
    accDescr: Tumult connects to the host, checks its host key, logs in with a key or the SSH agent, runs the command and reads back its output and exit code.
    step[Step in tumult] -->|connect, check host key| host[Remote host]
    host -->|stdout and exit code| step
```

## Run a command

```toon
method[1]:
  - name: restart-the-database
    activity_type: action
    provider:
      type: native
      plugin: tumult-ssh
      function: execute
      arguments:
        host: db-primary.example.com
        user: ops
        key_file: /home/ops/.ssh/tumult_ed25519
        command: systemctl restart postgresql
```

`execute` is the only function. Its arguments:

| Argument | Default | What it does |
|---|---|---|
| `host` | required | The host to connect to |
| `user` | required | The user to log in as |
| `command` | required | The command to run |
| `port` | `22` | The SSH port |
| `key_file` | the SSH agent | A private key. Without it, Tumult asks the agent that `SSH_AUTH_SOCK` names. |
| `host_key_policy` | `verify` | How to check the host key, see below |

The output of the command is the result of the step. A command that exits
with another code than 0 fails the step, with that code and its stderr.
Tumult waits up to 30 seconds for the connection and 60 seconds for the
command.

## Host keys

| Policy | What it does |
|---|---|
| `verify` | Checks the key against `~/.ssh/known_hosts` of the user that runs Tumult. An unknown or a changed key fails the step. |
| `trust-on-first-use` | Writes an unknown key to `known_hosts` the first time, then checks it |
| `accept-any` | Checks nothing. Use it only for a throwaway lab host. |

## Keys

Use an Ed25519 key, or an ECDSA key (P-256, P-384 or P-521). The key file
must be readable by its owner only (mode `0600`). A `key_file` with a
passphrase does not work: load that key into the SSH agent instead.

```bash
ssh-keygen -t ed25519 -f ~/.ssh/tumult_ed25519
```

RSA keys do not work. Tumult builds its SSH library without RSA, because
the `rsa` crate has a timing flaw with no fix
([RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071)).

## Next

- Run a plugin script on another host: copy it there and call it with `execute`.
- Use the SSH code from Rust: the [crate README](https://github.com/mwigge/tumult/blob/main/tumult-ssh/README.md).
