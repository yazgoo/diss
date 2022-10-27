<table>
<tr>
<td>
<img src=diss_logo.png width=100>
</td>
<td>
<b>diss</b>ociate (verb):
<pre>
1. (especially in abstract contexts) disconnect or separate.
</pre>
</td>
</tr>
</table>

[![Discord](https://img.shields.io/badge/discord--blue?logo=discord)](https://discord.gg/F684Y8rYwZ)

## What is diss ?

Diss:

- dissociates a program from current terminal
- is like [dtach](https://github.com/crigler/dtach), [abduco](https://github.com/martanne/abduco) (think [GNU screen](https://www.gnu.org/software/screen/) or [tmux](https://github.com/tmux/tmux) without multiplexing)
- is also a rust crate you can easily integrate

## How do I use diss CLI ?

### installing

1. [install cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html)
2. run `cargo install diss`

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
