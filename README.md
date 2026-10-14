# mini-sub2api

Connect Responses API clients to a Codex subscription or an OpenAI API key.

## Quick start

Install [mise](https://mise.jdx.dev/), then run:

```bash
mise install
bash scripts/build.sh

export MINI_SUB2API_STATE_DIR=./state
build/bin/mini-sub2api credential login codex --name personal
build/bin/mini-sub2api credential list
build/bin/mini-sub2api key create --credential cred_EXAMPLE --name laptop
build/bin/mini-sub2api serve
```

Replace `cred_EXAMPLE` with the credential ID from `credential list`.
Save the generated `ms2a_` key; it is shown only once.

Use `http://127.0.0.1:8787/v1` as the base URL and your `ms2a_` key for authentication.
See [Codex client setup](docs/OPERATIONS.md#codex-client).
Remote access requires [TLS configuration](docs/OPERATIONS.md#deployment).

## Documentation

- [Operations](docs/OPERATIONS.md): authentication, administration and deployment
- [Behavior and limits](docs/BEHAVIOR.md)
- [Codex compatibility](docs/CODEX_COMPATIBILITY.md) and [upgrade checklist](docs/CODEX_REANCHORING.md)

Personal learning/research software; not an official OpenAI product or intended for commercial/production use.
