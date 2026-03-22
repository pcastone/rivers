# Rivers v0.50.1 Release

## Features
- Declarative REST, WebSocket, SSE, Streaming REST, GraphQL endpoints via TOML
- 18 datasource drivers (postgres, mysql, sqlite, redis, kafka, rabbitmq, nats, mongodb, elasticsearch, cassandra, couchdb, influxdb, ldap, memcached, faker, eventbus, redis-streams, http)
- V8 JavaScript + Wasmtime WASM handler execution with capability model
- LockBox Age-encrypted secrets with zeroize-on-drop
- Mandatory TLS with auto-generated self-signed certificates
- Ed25519 admin API authentication with RBAC
- Session/CSRF management backed by StorageEngine (memory/sqlite/redis)
- DataView two-tier cache (L1 LRU + L2 StorageEngine) with declarative invalidation
- GraphQL: queries from DataViews, mutations from CodeComponent, subscriptions from EventBus
- SSE Last-Event-ID reconnection with bounded replay buffer
- Streaming REST multi-chunk generator protocol ({chunk, done})
- Polling with StorageEngine persistence and diff strategies (hash/null/change_detect)
- WebSocket lifecycle hooks (onConnect/onMessage/onDisconnect)
- Hot reload: views, DataViews, GraphQL schema rebuilt on config change
- Bundle validation: 9 checks at startup + riversctl validate CLI
- Config JSON Schema generation via schemars
- Health probes: per-datasource connectivity check with latency

## Binaries
- riversd — main server daemon
- riversctl — admin client + bundle validator + TLS management
- rivers-lockbox — encrypted keystore management
- riverpackage — bundle validation and packaging

## Documentation
- quickstart.md — 5-minute tutorial
- installation.md — full setup guide
- developer.md — app developer reference
- admin.md — operations reference
- cli.md — CLI reference
- rivers-skill.md — Claude AI skill for agentic development
- rivers-app-development.md — AI build spec
- rivers-v1-admin.md — AI operations spec
