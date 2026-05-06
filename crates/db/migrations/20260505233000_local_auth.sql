create table if not exists local_actor_credentials (
    actor_id uuid primary key references actors(id) on delete cascade,
    password_hash text not null
);

create table if not exists login_sessions (
    token text primary key,
    actor_id uuid not null references actors(id) on delete cascade,
    created_at timestamptz not null,
    expires_at timestamptz not null
);

create index if not exists login_sessions_actor_id_expires_at_idx
    on login_sessions(actor_id, expires_at desc);
