# Test Infrastructure — Podman Deployment

**Host:** 192.168.2.161 (CentOS Stream 9, x86_64, 128GB RAM, 4TB disk)
**Network:** macvlan `rivers-macvlan` on `eno2` (192.168.2.0/23)
**Access:** `ssh root@192.168.2.161`

---

## Service Map

| IP | Service | Cluster | Port | Credentials |
|----|---------|---------|------|-------------|
| 192.168.2.200 | Zookeeper 1 | ZK quorum (3) | 2181 | — |
| 192.168.2.201 | Zookeeper 2 | ZK quorum (3) | 2181 | — |
| 192.168.2.202 | Zookeeper 3 | ZK quorum (3) | 2181 | — |
| 192.168.2.203 | Kafka 1 | Broker cluster (3) | 9092 | — |
| 192.168.2.204 | Kafka 2 | Broker cluster (3) | 9092 | — |
| 192.168.2.205 | Kafka 3 | Broker cluster (3) | 9092 | — |
| 192.168.2.206 | Redis 1 | Redis Cluster (3) | 6379 | `rivers_test` |
| 192.168.2.207 | Redis 2 | Redis Cluster (3) | 6379 | `rivers_test` |
| 192.168.2.208 | Redis 3 | Redis Cluster (3) | 6379 | `rivers_test` |
| 192.168.2.209 | PostgreSQL 1 | Primary | 5432 | `rivers` / `rivers_test` / db: `rivers` |
| 192.168.2.210 | PostgreSQL 2 | Replica | 5432 | `rivers` / `rivers_test` / db: `rivers` |
| 192.168.2.211 | PostgreSQL 3 | Replica | 5432 | `rivers` / `rivers_test` / db: `rivers` |
| 192.168.2.212 | MongoDB 1 | Replica set `rivers-rs` (PRIMARY) | 27017 | `rivers` / `rivers_test` |
| 192.168.2.213 | MongoDB 2 | Replica set `rivers-rs` (SECONDARY) | 27017 | `rivers` / `rivers_test` |
| 192.168.2.214 | MongoDB 3 | Replica set `rivers-rs` (SECONDARY) | 27017 | `rivers` / `rivers_test` |
| 192.168.2.215 | MySQL 1 | InnoDB Cluster (3) | 3306 | `rivers` / `rivers_test` / db: `rivers` |
| 192.168.2.216 | MySQL 2 | InnoDB Cluster (3) | 3306 | `rivers` / `rivers_test` / db: `rivers` |
| 192.168.2.217 | MySQL 3 | InnoDB Cluster (3) | 3306 | `rivers` / `rivers_test` / db: `rivers` |
| 192.168.2.218 | Elasticsearch 1 | ES cluster `rivers-es` (3) | 9200 | security disabled |
| 192.168.2.219 | Elasticsearch 2 | ES cluster `rivers-es` (3) | 9200 | security disabled |
| 192.168.2.220 | Elasticsearch 3 | ES cluster `rivers-es` (3) | 9200 | security disabled |
| 192.168.2.221 | CouchDB 1 | CouchDB cluster (3) | 5984 | `rivers` / `rivers_test` |
| 192.168.2.222 | CouchDB 2 | CouchDB cluster (3) | 5984 | `rivers` / `rivers_test` |
| 192.168.2.223 | CouchDB 3 | CouchDB cluster (3) | 5984 | `rivers` / `rivers_test` |
| 192.168.2.224 | Cassandra 1 | Ring `rivers` (seed) | 9042 | — |
| 192.168.2.225 | Cassandra 2 | Ring `rivers` | 9042 | — |
| 192.168.2.226 | Cassandra 3 | Ring `rivers` | 9042 | — |
| 192.168.2.227 | LDAP | Single node | 389 | admin: `cn=admin,dc=rivers,dc=test` / `rivers_test` |
| 192.168.2.240 | Neo4j | Single node (community) | 7687 (Bolt), 7474 (HTTP) | `neo4j` / `rivers_test` / db: `neo4j` |

**28 containers, 9 clusters + 2 standalone**

---

## Connection Strings

### PostgreSQL
```
postgresql://rivers:rivers_test@192.168.2.209:5432/rivers
```

### MySQL
```
mysql://rivers:rivers_test@192.168.2.215:3306/rivers
```

### Redis Cluster
```
redis://:rivers_test@192.168.2.206:6379,192.168.2.207:6379,192.168.2.208:6379
```

### MongoDB Replica Set
```
mongodb://rivers:rivers_test@192.168.2.212:27017,192.168.2.213:27017,192.168.2.214:27017/?replicaSet=rivers-rs&authSource=admin
```

### Kafka Bootstrap
```
192.168.2.203:9092,192.168.2.204:9092,192.168.2.205:9092
```

### Zookeeper Connect
```
192.168.2.200:2181,192.168.2.201:2181,192.168.2.202:2181
```

### Elasticsearch
```
http://192.168.2.218:9200,http://192.168.2.219:9200,http://192.168.2.220:9200
```

### CouchDB
```
http://rivers:rivers_test@192.168.2.221:5984
```

### Cassandra
```
192.168.2.224:9042,192.168.2.225:9042,192.168.2.226:9042
```

### LDAP
```
ldap://192.168.2.227:389
Base DN: dc=rivers,dc=test
Bind DN: cn=admin,dc=rivers,dc=test
Password: rivers_test
```

---

## Quick Verification

```bash
# From Mac (all services reachable on LAN)
curl -s http://192.168.2.218:9200/_cluster/health?pretty    # ES
curl -s http://rivers:rivers_test@192.168.2.221:5984/       # CouchDB
redis-cli -h 192.168.2.206 -a rivers_test cluster info      # Redis
psql -h 192.168.2.209 -U rivers -d rivers -c "SELECT 1"    # PostgreSQL
mysql -h 192.168.2.215 -u rivers -privers_test rivers -e "SELECT 1"  # MySQL
mongosh "mongodb://rivers:rivers_test@192.168.2.212:27017/?authSource=admin" --eval "rs.status().ok"  # MongoDB
```

## Container Management

```bash
# SSH to host
ssh root@192.168.2.161

# Status
podman ps --format "{{.Names}}\t{{.Status}}" | grep rivers | sort

# Stop all
podman stop $(podman ps -q --filter name=rivers-)

# Start all
podman start $(podman ps -aq --filter name=rivers-)

# Remove all
podman rm -f $(podman ps -aq --filter name=rivers-)
podman network rm rivers-macvlan
```

---

## Notes

- **macvlan limitation:** The Podman host (192.168.2.161) cannot reach container IPs directly. Use `podman exec` from the host, or connect from any other machine on the LAN.
- **Persistence:** Containers use ephemeral storage. Data is lost on `podman rm`. For persistent test data, add `-v` volume mounts.
- **Images:** All from `docker.io/library/` (official) except Kafka (`confluentinc/cp-kafka:7.6.0`) and LDAP (`osixia/openldap:1.5.0`).
