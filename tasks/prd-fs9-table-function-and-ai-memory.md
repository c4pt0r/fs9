[PRD]
# PRD: fs9 Table Function & AI Memory Protocol

## Overview

为 fs9 分布式文件系统添加 SQL table function 能力（`fs9()`），使文件系统中的文件和目录可以通过 SQL 直接查询。在此基础上，定义一套通用的 AI Memory 文件协议接口，使任意 AI agent 框架可以利用 fs9 的文件读写 + SQL 查询作为持久化记忆层。

核心设计原则：**文件即表，目录即 catalog，记忆即文件**。Memory 层不发明新抽象，而是通过 fs9() table function 的原子能力组合实现。

## Goals

- 提供统一的 `fs9()` 函数接口，路径指向目录返回元数据表，指向文件返回内容表
- 支持 CSV、JSONL、raw text 三种格式解码，按扩展名自动推断
- 支持 glob pattern 实现跨文件联合查询
- 定义 SQL-engine-agnostic 的接口规范，不绑定具体数据库实现
- 定义 AI Memory 协议接口（store / recall / forget / consolidate），完全基于 fs9 文件读写 + fs9() 查询
- 第一版为只读查询 + glob，写回（INSERT INTO fs9）作为后续迭代

## Quality Gates

以下命令必须对每个 user story 通过：
- `make test`

## SQL Examples

以下示例贯穿整个 PRD，展示 fs9() 在不同场景下的用法。

### 目录查询

```sql
-- 列出目录内容
SELECT path, type, size, mtime
FROM fs9('/data/logs/')
WHERE type = 'file' AND size > 1024
ORDER BY mtime DESC;

-- 递归查找所有 CSV 文件
SELECT path, size
FROM fs9('/data/', recursive => true)
WHERE path LIKE '%.csv'
ORDER BY size DESC
LIMIT 10;
```

### 文件查询 — CSV

```sql
-- CSV 文件直接当表查
SELECT customer_id, SUM(amount) AS total
FROM fs9('/data/sales.csv')
WHERE region = 'APAC'
GROUP BY customer_id
ORDER BY total DESC;

-- TSV，显式指定分隔符
SELECT *
FROM fs9('/data/export.tsv', format => 'csv', delimiter => '\t');

-- 无 header 的 CSV，手动指定列名
SELECT *
FROM fs9('/data/raw.dat',
         format => 'csv',
         columns => 'id INT, ts DATETIME, val DOUBLE',
         header => false);
```

### 文件查询 — JSONL

```sql
-- 日志分析：按 level 聚合
SELECT
  json_extract(line, '$.level') AS level,
  COUNT(*) AS cnt
FROM fs9('/logs/app.jsonl')
GROUP BY level;

-- 从 JSONL 提取结构化字段（schema 自动推断）
SELECT timestamp, level, message, service
FROM fs9('/logs/structured.jsonl')
WHERE level = 'ERROR'
  AND timestamp > '2024-02-01'
ORDER BY timestamp DESC
LIMIT 100;
```

### 文件查询 — Raw Text 兜底

```sql
-- 任意文件逐行查
SELECT line_number, line
FROM fs9('/etc/hosts')
WHERE line NOT LIKE '#%' AND line != '';

-- 结合字符串函数做 ad-hoc parsing
SELECT
  split_part(line, ':', 1) AS username,
  split_part(line, ':', 3) AS uid
FROM fs9('/etc/passwd')
WHERE CAST(split_part(line, ':', 3) AS INT) < 1000;
```

### Glob — 跨文件聚合

```sql
-- 所有月份的销售数据，一条 SQL
SELECT
  _path,
  COUNT(*) AS rows,
  SUM(amount) AS total
FROM fs9('/data/sales/2024-*/*.csv')
GROUP BY _path
ORDER BY total DESC;

-- 跨多个日志文件搜索错误
SELECT
  _path AS source_file,
  _line_number,
  json_extract(line, '$.message') AS error_msg,
  json_extract(line, '$.timestamp') AS ts
FROM fs9('/logs/**/*.jsonl')
WHERE json_extract(line, '$.level') = 'ERROR'
ORDER BY ts DESC
LIMIT 50;
```

### AI Memory — 情景记忆（Episodic）

```sql
-- 查找与某主题相关的历史对话
SELECT
  _path AS conversation,
  json_extract(line, '$.timestamp') AS ts,
  json_extract(line, '$.role') AS role,
  json_extract(line, '$.content') AS content
FROM fs9('/agent/jarvis/episodes/*.jsonl')
WHERE line LIKE '%数据库%'
ORDER BY ts DESC
LIMIT 20;

-- 最近 7 天与用户的所有对话摘要
SELECT
  _path AS session,
  COUNT(*) AS message_count,
  MIN(json_extract(line, '$.timestamp')) AS started_at,
  MAX(json_extract(line, '$.timestamp')) AS ended_at
FROM fs9('/agent/jarvis/episodes/2024-02-*.jsonl')
GROUP BY _path
ORDER BY started_at DESC;
```

### AI Memory — 语义记忆（Semantic）

```sql
-- 查询某个用户相关的所有已知事实
SELECT fact, confidence, source, learned_at
FROM fs9('/agent/jarvis/knowledge/user_dongxu.jsonl')
WHERE confidence > 0.5
ORDER BY confidence DESC;

-- 跨所有知识文件搜索
SELECT _path, fact, confidence
FROM fs9('/agent/jarvis/knowledge/*.jsonl')
WHERE fact LIKE '%TiKV%'
ORDER BY learned_at DESC;
```

### AI Memory — 记忆巩固（Consolidation）

```sql
-- 找出超过 7 天未巩固的对话（用于触发 consolidation pipeline）
SELECT _path, COUNT(*) AS msg_count
FROM fs9('/agent/jarvis/episodes/*.jsonl')
WHERE _path NOT IN (
  SELECT source FROM fs9('/agent/jarvis/knowledge/_consolidated.csv')
)
GROUP BY _path
HAVING MAX(json_extract(line, '$.timestamp')) < NOW() - INTERVAL 7 DAY;

-- 记忆衰减：降低长时间未被引用的事实的 confidence
-- （此为写回场景，在后续 INSERT INTO fs9 迭代中实现）
-- SELECT fact, confidence * 0.8 AS new_confidence
-- FROM fs9('/agent/jarvis/knowledge/user_dongxu.jsonl')
-- WHERE last_recalled < NOW() - INTERVAL 30 DAY;
```

### 组合查询 — 文件元数据 + 内容

```sql
-- "最近 7 天哪些 CSV 文件行数异常多？"
SELECT
  meta.path,
  meta.size,
  meta.mtime,
  content.row_count
FROM fs9('/data/', recursive => true) AS meta
CROSS JOIN LATERAL (
  SELECT COUNT(*) AS row_count
  FROM fs9(meta.path)
) AS content
WHERE meta.path LIKE '%.csv'
  AND meta.mtime > NOW() - INTERVAL 7 DAY
  AND content.row_count > 100000
ORDER BY content.row_count DESC;
```

## User Stories

### US-001: FormatDecoder trait 与 raw text 解码器

**Description:** As a developer, I want a `FormatDecoder` trait that defines how file bytes are decoded into rows, and a raw text fallback decoder, so that any file can be queried as a table at minimum as line-by-line text.

**SQL Example:**
```sql
-- 任意文件都能查，最差情况就是逐行文本
SELECT line_number, line
FROM fs9('/var/log/syslog')
WHERE line LIKE '%error%';
```

**Acceptance Criteria:**
- [ ] 定义 `FormatDecoder` trait，接口包含：`decode(reader) -> Iterator<Row>` 和 `schema() -> Vec<Column>`
- [ ] `Row` 类型支持 `String`、`i64`、`f64`、`bool`、`null` 值
- [ ] 实现 `RawTextDecoder`，输出 schema 为 `(line_number: i64, line: String)`
- [ ] 任意文件（包括二进制）使用 `RawTextDecoder` 时不会 panic
- [ ] 单元测试覆盖空文件、单行、多行、超长行场景

### US-002: CSV 解码器

**Description:** As a developer, I want a CSV format decoder that can infer column names from headers and parse rows into typed columns, so that CSV files can be queried as structured tables.

**SQL Example:**
```sql
-- 标准 CSV
SELECT name, age, city FROM fs9('/data/users.csv') WHERE age > 18;

-- TSV 无 header
SELECT col_0 AS id, col_1 AS value
FROM fs9('/data/export.tsv', format => 'csv', delimiter => '\t', header => false);
```

**Acceptance Criteria:**
- [ ] 实现 `CsvDecoder`，支持参数：`header: bool`（默认 true）、`delimiter: char`（默认 ','）
- [ ] 当 `header=true` 时，第一行作为列名；当 `header=false` 时，列名为 `col_0, col_1, ...`
- [ ] 支持通过 `columns` 参数显式指定列名和类型，覆盖自动推断
- [ ] 正确处理带引号的字段（含逗号、换行）
- [ ] 单元测试覆盖：标准 CSV、TSV（delimiter='\t'）、无 header、空文件、引号字段

### US-003: JSONL 解码器

**Description:** As a developer, I want a JSONL format decoder that treats each line as a JSON object and exposes top-level keys as columns, so that JSON Lines log files and data files can be queried as tables.

**SQL Example:**
```sql
-- JSONL 日志，top-level keys 自动成为列名
SELECT timestamp, level, message
FROM fs9('/logs/app.jsonl')
WHERE level = 'ERROR'
ORDER BY timestamp DESC;

-- 嵌套字段保持为 JSON string
SELECT
  user_id,
  json_extract(metadata, '$.ip') AS ip
FROM fs9('/logs/access.jsonl');
```

**Acceptance Criteria:**
- [ ] 实现 `JsonlDecoder`，每行 parse 为 JSON object
- [ ] 顶层 key 作为列名，value 根据 JSON 类型映射：string→String, number→f64/i64, bool→bool, null→null
- [ ] 嵌套 object/array 保持为 JSON string（不展开）
- [ ] Schema 通过扫描前 N 行（默认 100）推断，合并所有出现的 key
- [ ] 某行缺少某个 key 时该列值为 null
- [ ] 单元测试覆盖：统一 schema、混合 key、嵌套值、空行跳过、非法 JSON 行跳过

### US-004: 格式自动检测

**Description:** As a developer, I want the system to auto-detect file format from extension and support explicit override, so that users don't need to specify format for common file types.

**SQL Example:**
```sql
-- 自动推断：.csv → CSV decoder
SELECT * FROM fs9('/data/sales.csv');

-- 自动推断：.jsonl → JSONL decoder
SELECT * FROM fs9('/logs/app.jsonl');

-- 显式覆盖：把 .log 文件当 JSONL 解析
SELECT * FROM fs9('/logs/app.log', format => 'jsonl');

-- 无法识别的扩展名 → raw text 兜底
SELECT line FROM fs9('/data/mystery.dat');
```

**Acceptance Criteria:**
- [ ] 扩展名映射：`.csv` → CsvDecoder, `.tsv` → CsvDecoder(delimiter='\t'), `.jsonl`/`.ndjson` → JsonlDecoder, 其他 → RawTextDecoder
- [ ] 支持 `format` 参数显式指定，优先级高于扩展名推断
- [ ] 当显式指定不支持的 format 时，返回明确错误
- [ ] 单元测试覆盖所有扩展名映射和 override 场景

### US-005: 目录查询 — 元数据表

**Description:** As a user, I want `fs9('/some/dir/')` to return a metadata table of all entries in that directory, so that I can query file metadata using SQL.

**SQL Example:**
```sql
-- 列出目录
SELECT * FROM fs9('/data/');
-- 返回:
-- | path              | type | size   | mode | mtime               |
-- |-------------------|------|--------|------|---------------------|
-- | /data/sales.csv   | file | 102400 | 0644 | 2024-02-07 10:30:00 |
-- | /data/logs/       | dir  |      0 | 0755 | 2024-02-07 09:00:00 |

-- 递归找出最大的 10 个文件
SELECT path, size FROM fs9('/data/', recursive => true)
WHERE type = 'file'
ORDER BY size DESC
LIMIT 10;
```

**Acceptance Criteria:**
- [ ] 路径以 `/` 结尾或 stat 结果为 directory 时，进入目录模式
- [ ] 返回固定 schema：`(path: String, type: String, size: i64, mode: i64, mtime: String)`
- [ ] `type` 值为 `"file"` 或 `"dir"`
- [ ] 支持 `recursive` 参数（默认 false），为 true 时递归列出所有后代
- [ ] 结果按 `path` 字典序排序
- [ ] 通过 fs9 HTTP API（GET /readdir）获取数据
- [ ] 集成测试：创建含文件和子目录的目录结构，验证返回结果

### US-006: 文件查询 — 内容表

**Description:** As a user, I want `fs9('/data/file.csv')` to return file contents as a table with auto-detected format, so that I can query file data using SQL.

**SQL Example:**
```sql
-- 读 CSV 文件，自动识别 header
SELECT customer_id, amount
FROM fs9('/data/sales.csv')
WHERE amount > 1000;

-- 读 JSONL 文件
SELECT level, message
FROM fs9('/logs/app.jsonl')
WHERE level IN ('ERROR', 'FATAL');

-- 隐藏列 _line_number 可用于定位
SELECT _line_number, line
FROM fs9('/data/raw.txt')
WHERE line LIKE '%WARN%';
```

**Acceptance Criteria:**
- [ ] 路径指向文件时，进入文件模式
- [ ] 根据 US-004 的规则选择 FormatDecoder
- [ ] 通过 fs9 HTTP API（open → read → close）获取文件内容
- [ ] 每行自动附加隐藏列 `_line_number: i64`（从 1 开始）
- [ ] 对空文件返回空结果集（0 行），schema 仍然可用
- [ ] 集成测试：写入已知内容的 CSV/JSONL/纯文本文件，通过 fs9() 查询并验证结果

### US-007: Glob 展开与多文件联合查询

**Description:** As a user, I want `fs9('/logs/2024-*/*.jsonl')` to query across multiple matching files as a single table, so that I can aggregate data from many files in one query.

**SQL Example:**
```sql
-- 跨月份聚合销售数据
SELECT
  _path AS source,
  COUNT(*) AS rows,
  SUM(amount) AS total
FROM fs9('/data/sales/2024-*/*.csv')
GROUP BY _path;

-- 跨所有子目录搜索错误日志
SELECT _path, _line_number, message
FROM fs9('/logs/**/*.jsonl')
WHERE level = 'ERROR'
ORDER BY timestamp DESC;

-- 无匹配时返回空结果，不报错
SELECT * FROM fs9('/data/nonexistent-*.csv');
-- → 0 rows
```

**Acceptance Criteria:**
- [ ] 路径包含 `*` 或 `**` 时进入 glob 模式
- [ ] 使用 readdir 递归展开 glob pattern，匹配文件列表
- [ ] 所有匹配文件使用相同 FormatDecoder（由第一个文件推断，或显式指定）
- [ ] 结果包含额外隐藏列 `_path: String`（标识每行来源文件）
- [ ] Schema 取所有文件的 schema 并集（缺失列为 null）
- [ ] 无匹配文件时返回空结果集，不报错
- [ ] 集成测试：创建多个文件，用 glob 查询并验证 `_path` 列和行数

### US-008: 查询参数接口定义

**Description:** As a developer, I want a well-defined parameter interface for fs9() that is SQL-engine-agnostic, so that any database can implement this function using the same spec.

**HTTP API Example:**
```bash
# 查询 CSV 文件
curl -X POST http://localhost:3000/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{
    "path": "/data/sales.csv",
    "format": "csv",
    "header": true
  }'

# 响应
{
  "schema": [
    {"name": "customer_id", "type": "string"},
    {"name": "amount", "type": "float"},
    {"name": "region", "type": "string"}
  ],
  "rows": [
    ["C001", 150.0, "APAC"],
    ["C002", 280.5, "EU"]
  ],
  "metadata": {
    "files_scanned": 1,
    "rows_returned": 2,
    "bytes_read": 4096
  }
}
```

```bash
# Glob 查询
curl -X POST http://localhost:3000/api/v1/query \
  -d '{"path": "/logs/**/*.jsonl", "format": "jsonl"}'

# 目录查询
curl -X POST http://localhost:3000/api/v1/query \
  -d '{"path": "/data/", "recursive": true}'
```

**Acceptance Criteria:**
- [ ] 定义 `Fs9QueryRequest` 结构：`path: String`, `format: Option<String>`, `columns: Option<Vec<ColumnDef>>`, `delimiter: Option<char>`, `header: Option<bool>`, `recursive: Option<bool>`
- [ ] 定义 `Fs9QueryResponse` 结构：`schema: Vec<Column>`, `rows: Vec<Row>`, `metadata: QueryMetadata`（含 files_scanned, rows_returned, bytes_read）
- [ ] 在 fs9-server 上实现 HTTP endpoint `POST /api/v1/query`，接受 JSON 格式的 `Fs9QueryRequest`，返回 `Fs9QueryResponse`
- [ ] 生成 OpenAPI/JSON Schema 文档描述接口
- [ ] 集成测试通过 HTTP 调用验证完整 request/response cycle

### US-009: AI Memory 协议接口定义

**Description:** As an AI framework developer, I want a documented memory protocol that maps memory operations (store, recall, forget, consolidate) to fs9 file operations and fs9() queries, so that I can build agent memory on fs9 without being locked to any specific directory structure or framework.

**Usage Example — Store (写入记忆):**
```bash
# 存储一条情景记忆（追加到对话日志）
curl -X POST http://localhost:3000/api/v1/write \
  -d '{
    "path": "/agent/jarvis/episodes/2024-02-07_session1.jsonl",
    "mode": "append",
    "data": "{\"timestamp\":\"2024-02-07T10:30:00Z\",\"role\":\"user\",\"content\":\"帮我查看TiKV的连接状态\"}\n"
  }'
```

**Usage Example — Recall (回忆查询):**
```sql
-- 回忆与 TiKV 相关的所有对话
SELECT
  _path AS session,
  timestamp, role, content
FROM fs9('/agent/jarvis/episodes/*.jsonl')
WHERE content LIKE '%TiKV%'
ORDER BY timestamp DESC
LIMIT 10;

-- 查询高置信度的事实
SELECT fact, confidence, learned_at
FROM fs9('/agent/jarvis/knowledge/*.jsonl')
WHERE confidence > 0.7
ORDER BY learned_at DESC;
```

**Usage Example — Forget (遗忘):**
```bash
# 删除整个旧对话文件
curl -X DELETE http://localhost:3000/api/v1/fs/agent/jarvis/episodes/2023-ancient.jsonl

# 行级遗忘：查询出要保留的行，重写文件（后续 INSERT INTO 迭代实现）
```

**Usage Example — Consolidate (巩固):**
```sql
-- Step 1: 找到需要巩固的旧对话
SELECT _path
FROM fs9('/agent/jarvis/episodes/')
WHERE mtime < NOW() - INTERVAL 7 DAY;

-- Step 2: 读取对话内容，送给 LLM 提取事实
SELECT content
FROM fs9('/agent/jarvis/episodes/2024-01-30_session3.jsonl')
WHERE role = 'assistant';

-- Step 3: LLM 提取结果写入知识库（store 操作）
-- Step 4: 原对话文件可归档或删除
```

**Acceptance Criteria:**
- [ ] 定义四个核心操作的接口映射：
  - `store(content, metadata)` → fs9 write（创建/追加文件）
  - `recall(query)` → fs9() table function 查询
  - `forget(criteria)` → fs9 remove（删除文件或文件中的行）
  - `consolidate(source, target)` → fs9() 读取 + 处理 + fs9 write
- [ ] 每个操作定义 HTTP API 对应关系（用已有的 fs9 REST API 组合，无需新增 endpoint）
- [ ] 提供三个记忆类型的参考 pattern（非强制规范）：
  - 情景记忆（Episodic）：对话日志存为 JSONL，按时间查询
  - 语义记忆（Semantic）：事实提取存为 JSONL，按 confidence 查询
  - 工作记忆（Working）：当前上下文存为 JSON，直接读取
- [ ] 每个 pattern 包含完整的 SQL 查询示例
- [ ] 定义 memory entry 的最小 schema 建议（非强制）：`timestamp`, `content`, `source`, `type`
- [ ] 文档以 Markdown 形式存放在 `docs/memory-protocol.md`

### US-010: Memory 协议参考实现 — Rust client 封装

**Description:** As an AI agent developer, I want a thin Rust client library that wraps the memory protocol operations on top of the existing `Fs9Client`, so that agents can store/recall/forget memories without manually constructing HTTP requests.

**API Example:**
```rust
let mem = MemoryClient::new(fs9_client);

// Store: 追加一条情景记忆
mem.store("/agent/jarvis/episodes/2024-02-07.jsonl", vec![
    MemoryEntry {
        timestamp: Utc::now(),
        content: "用户偏好 RawClient 而非 TransactionClient".into(),
        metadata: json!({"type": "preference", "confidence": 0.9}),
    },
]).await?;

// Recall: 查询相关记忆
let results = mem.recall(RecallRequest {
    path: "/agent/jarvis/knowledge/*.jsonl".into(),
    filter: Some("confidence > 0.5".into()),
    limit: Some(20),
}).await?;

for entry in results {
    println!("{}: {} (confidence: {})",
        entry.timestamp, entry.content,
        entry.metadata["confidence"]);
}

// Forget: 删除旧对话
mem.forget("/agent/jarvis/episodes/2023-old-session.jsonl").await?;
```

**Acceptance Criteria:**
- [ ] 在 `clients/rust/` 中新增 `memory` module
- [ ] 实现 `MemoryClient` struct，包装 `Fs9Client`
- [ ] `store(path, entries: Vec<MemoryEntry>)` — 将 entries 序列化为 JSONL 追加写入指定路径
- [ ] `recall(path_or_glob, filter: Option<RecallFilter>)` — 调用 `POST /api/v1/query` 读取并返回结构化结果
- [ ] `forget(path, criteria)` — 通过 query + 重写实现行级删除（或整文件删除）
- [ ] `MemoryEntry` 结构体包含：`timestamp: DateTime`, `content: String`, `metadata: serde_json::Value`
- [ ] 单元测试 mock HTTP 验证序列化格式
- [ ] 集成测试对活的 fs9-server 验证完整 store → recall → forget 流程

## Functional Requirements

- FR-1: `fs9()` 函数接受路径字符串作为第一个参数，返回行列形式的结果集
- FR-2: 路径指向目录时返回固定 schema 的元数据行，路径指向文件时返回解码后的内容行
- FR-3: 格式检测优先级：显式 `format` 参数 > 文件扩展名 > raw text 兜底
- FR-4: Glob pattern 中的 `*` 匹配单层文件名，`**` 匹配多层路径
- FR-5: 每个内容行携带 `_line_number` 隐藏列；glob 模式下额外携带 `_path` 隐藏列
- FR-6: JSONL decoder 通过前 100 行采样推断 schema，缺失字段填 null
- FR-7: CSV decoder 支持自定义分隔符和 header 开关
- FR-8: 查询接口通过 HTTP `POST /api/v1/query` 暴露，请求/响应均为 JSON
- FR-9: Memory 协议操作完全映射到已有 fs9 REST API，不引入新的服务端状态
- FR-10: Memory client 的 `store` 操作以 append 模式写入，不覆盖已有内容

## Non-Goals (Out of Scope)

- **写回能力**：第一版不支持 `INSERT INTO fs9()`，仅支持只读查询。写回作为后续迭代
- **SQL 引擎绑定**：不实现 TiDB coprocessor、DuckDB extension 等具体集成。只定义 HTTP 接口
- **Parquet/Arrow 格式**：第一版不支持二进制列式格式
- **向量检索/Embedding**：不内置 cosine similarity 等向量操作。Agent 可自行在应用层实现
- **目录结构规范**：Memory 协议不强制任何目录命名约定
- **实时订阅**：不在 table function 中集成 pubsubfs 的订阅能力
- **Predicate pushdown**：第一版为全量读取后过滤，不做服务端谓词下推优化
- **认证/权限**：复用 fs9 已有的 JWT 认证，不额外设计 memory 级别的 ACL

## Technical Considerations

- FormatDecoder 实现为独立 crate（如 `fs9-format`），可被 server 和 client 共同依赖
- `POST /api/v1/query` handler 在 `server/src/api/` 下新增，复用已有的 VfsRouter 进行文件访问
- Glob 展开通过递归 readdir + pattern match 实现，复用 `core/` 的 path resolution
- JSONL schema 推断需缓冲前 N 行，大文件场景需注意内存控制
- `_path` 和 `_line_number` 作为隐藏列，在 schema 中标记为 `hidden: true`，默认不返回除非显式 SELECT
- Memory client 的 `forget` 操作涉及"读取 → 过滤 → 重写"，非原子操作，文档需说明一致性限制
- 第一版不做流式返回，整个结果集在 server 端组装后一次性返回。大文件场景需设置合理的行数限制（默认 10000 行）

## Success Metrics

- 所有 FormatDecoder 单元测试通过，覆盖正常和边界场景
- 通过 HTTP API 可以成功查询 CSV、JSONL、纯文本文件并得到正确的结构化结果
- Glob 查询可以跨 10+ 文件聚合数据并返回正确的 `_path` 标识
- Memory client 可以完成 store → recall → forget 完整流程
- `make test` 通过

## Open Questions

- 行数限制（默认 10000）是否需要支持分页/cursor？如需要，在后续迭代中加入
- JSONL schema 推断的采样行数（100）是否需要可配置？
- Memory client 的 `forget` 操作的非原子性是否在第一版就需要解决？或在文档中标注即可？
- 是否需要提供 Python client 的 memory 封装？（当前 `clients/python/` 已存在基础 client）

[/PRD]
