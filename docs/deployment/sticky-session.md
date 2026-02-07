# FS9 Sticky Session Deployment Guide

## Problem

FS9 keeps file handles in memory. When running multiple instances behind a load balancer, a client's requests must consistently route to the same instance — otherwise, handles opened on Instance A won't exist on Instance B.

## Strategy

Route all requests for the same namespace to the same server instance using consistent hashing on the `Authorization` header (which contains the JWT with the `ns` claim).

```
                ┌──────────────────┐
                │   Load Balancer  │
                │  hash(namespace) │──► fixed instance
                └──────┬───────────┘
                       │
            ┌──────────┼──────────┐
            │          │          │
     ┌──────▼──┐ ┌─────▼───┐ ┌───▼──────┐
     │ Server1 │ │ Server2 │ │ Server3  │
     └─────────┘ └─────────┘ └──────────┘
```

## Nginx Configuration

```nginx
upstream fs9_servers {
    hash $http_authorization consistent;

    server fs9-1:9999;
    server fs9-2:9999;
    server fs9-3:9999;
}

server {
    listen 443 ssl http2;
    server_name fs9.example.com;

    location / {
        proxy_pass http://fs9_servers;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_connect_timeout 5s;
        proxy_read_timeout 30s;
        proxy_send_timeout 30s;
        client_max_body_size 256M;
    }

    location /health {
        proxy_pass http://fs9_servers;
        access_log off;
    }
}
```

## Envoy Configuration

```yaml
static_resources:
  listeners:
  - name: fs9_listener
    address:
      socket_address:
        address: 0.0.0.0
        port_value: 443
    filter_chains:
    - filters:
      - name: envoy.filters.network.http_connection_manager
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          route_config:
            virtual_hosts:
            - name: fs9
              domains: ["*"]
              routes:
              - match:
                  prefix: "/"
                route:
                  cluster: fs9_cluster
                  hash_policy:
                  - header:
                      header_name: Authorization

  clusters:
  - name: fs9_cluster
    type: STRICT_DNS
    lb_policy: RING_HASH
    load_assignment:
      cluster_name: fs9_cluster
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: fs9-1
                port_value: 9999
        - endpoint:
            address:
              socket_address:
                address: fs9-2
                port_value: 9999
        - endpoint:
            address:
              socket_address:
                address: fs9-3
                port_value: 9999
    health_checks:
    - timeout: 5s
      interval: 10s
      healthy_threshold: 2
      unhealthy_threshold: 3
      http_health_check:
        path: /health
```

## Health Endpoint

The `/health` endpoint returns JSON with an `instance_id` field for debugging routing:

```json
{
  "status": "ok",
  "instance_id": "a1b2c3d4"
}
```

Use this to verify that requests from the same client consistently route to the same instance.

## Monitoring

Use the `fs9_active_handles` Prometheus metric (per-namespace gauge) to detect hot spots. If one instance has significantly more handles than others, consider namespace rebalancing.
