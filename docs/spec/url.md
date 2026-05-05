# URL仕様

この文書は、2026-05-05 時点の実装から分かる URL 仕様だけを書いたものです。

まだ router や ActivityPub endpoint はほとんど未実装なので、以下を明確に分けます。

- すでにコードで確定している URL
- 型として存在するが、生成規則が未確定な URL

## 確定している URL

## 1. ヘルスチェック

`crates/server/src/main.rs` で、以下の route が実装されています。

- `GET /health`

現状は固定文字列 `ok` を返します。

## 2. 投稿 URL

`crates/domain/src/lib.rs` の `Post::new(new_post, public_base_url)` で、投稿 URL は次の形で生成されます。

```txt
{public_base_url}/posts/{post_id}
```

具体例:

```txt
https://example.invalid/posts/01974f87-2f40-7d11-a5aa-8a9d6d9d9a3b
```

補足:

- `post_id` は `Uuid::now_v7()` で生成されます
- `public_base_url` の末尾 `/` は `trim_end_matches('/')` で除去されます
- そのため `https://example.invalid` と `https://example.invalid/` は同じ結果になります

例:

```txt
public_base_url = https://example.invalid
-> https://example.invalid/posts/{post_id}

public_base_url = https://example.invalid/
-> https://example.invalid/posts/{post_id}
```

## 型として存在するが未確定の URL

## 1. actor URL

`ActorProfile` には以下の field があります。

- `actor_url: Url`
- `inbox_url: Option<Url>`
- `outbox_url: Option<Url>`

ただし現時点では、これらをどのパターンで生成するかは実装されていません。  
つまり、URL を保持する型はあるが、URL 規約そのものはまだ固定されていません。

現時点で言えるのは以下だけです。

- local actor も remote actor も `ActorProfile` を持つ
- ActivityPub 変換では `actor_url` が actor の `id` に使われる

## 2. inbox / outbox URL

`ActorProfile` に `inbox_url`, `outbox_url` があるため、domain としては actor ごとに inbox/outbox を持てる前提です。

ただし以下はまだ未確定です。

- local actor の inbox を `/users/{username}/inbox` にするか
- shared inbox を `/inbox` にするか
- outbox を `/users/{username}/outbox` にするか

これらは `server` の route 実装時に確定します。

## 現時点で未確定なもの

以下はドキュメント上では候補が出ていますが、コードとしてはまだ確定していません。

- actor ページ URL
- WebFinger endpoint
- inbox endpoint
- outbox endpoint
- followers endpoint
- following endpoint
- media URL

たとえば `tmp/docs/implementation-plan.md` では次が候補として挙がっています。

- `GET /.well-known/webfinger`
- `GET /users/:name`
- `GET /users/:name/outbox`
- `POST /users/:name/inbox`
- `GET /posts/:id`

ただし、これは現時点では実装計画であり、まだ仕様確定とはみなしません。

## 現状の暫定ルール

今の実装だけから安全に言える暫定ルールは次の通りです。

- サーバーの base URL は `https://example.invalid` のような origin 単位で与える
- 投稿 URL は常に `/posts/{uuidv7}` 形式
- actor 系 URL は field として存在するが、パス規約は未確定

## 今後ここで固定すべき項目

URL 仕様として次に固定するべきなのは以下です。

1. local actor の canonical URL
2. local actor の inbox / outbox URL
3. WebFinger の `resource=acct:{username}@{host}` から引く actor URL
4. shared inbox を持つかどうか
5. media attachment URL

現状の実装に一番近い候補は以下です。

```txt
GET  /health
GET  /users/{username}
GET  /users/{username}/outbox
POST /users/{username}/inbox
GET  /posts/{post_id}
```

ただし、`/users/` を最終採用するかはまだ未決定です。
