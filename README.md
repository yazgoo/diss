# diss

![diss](diss.png | width=100)

[![Discord](https://img.shields.io/badge/discord--blue?logo=discord)](https://discord.gg/F684Y8rYwZ)


```
dissocate
verb
1. (especially in abstract contexts) disconnect or separate.
```

## What is diss ?

Diss:

- dissociates a program from current terminal
- is like dtach, abduco (think GNU screen without multiplexing)
- is also a rust crate you can easily integrate

## How do I use diss CLI ?

### create session (or reatach if already exists), detach with CTRL+g

```
diss -e g -a session-name vim hello
```

### attach to running session

```
diss -e g -a session-name
```

### list running sessions

```
diss -l
```

## projects based on diss

- [vmux](https://github.com/yazgoo/vmux), a vim terminal multiplexer
