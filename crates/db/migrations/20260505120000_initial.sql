create table if not exists actors (
    id uuid primary key,
    username text not null,
    display_name text not null,
    summary text,
    actor_url text not null unique,
    inbox_url text,
    outbox_url text,
    created_at timestamptz not null default current_timestamp
);

create index if not exists actors_username_idx on actors(username);

create table if not exists local_actor_secrets (
    actor_id uuid primary key references actors(id) on delete cascade,
    public_key_pem text not null,
    private_key_pem text not null
);

create table if not exists remote_actor_state (
    actor_id uuid primary key references actors(id) on delete cascade,
    public_key_pem text,
    fetched_at timestamptz not null
);

create table if not exists posts (
    id uuid primary key,
    actor_id uuid not null references actors(id),
    url text not null unique,
    content_source text not null,
    content_format text not null,
    content_html text not null,
    visibility text not null,
    in_reply_to uuid references posts(id),
    created_at timestamptz not null
);

create index if not exists posts_actor_id_created_at_idx
    on posts(actor_id, created_at desc);

create index if not exists posts_in_reply_to_idx
    on posts(in_reply_to);
