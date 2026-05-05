create table if not exists follows (
    local_actor_id uuid not null references actors(id) on delete cascade,
    remote_actor_id uuid not null references actors(id) on delete cascade,
    remote_actor_url text not null,
    state text not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    primary key (local_actor_id, remote_actor_id)
);

create index if not exists follows_local_actor_state_idx
    on follows(local_actor_id, state);

create table if not exists delivery_jobs (
    id uuid primary key,
    local_actor_id uuid not null references actors(id) on delete cascade,
    target_inbox_url text not null,
    kind text not null,
    payload text not null,
    state text not null,
    attempts integer not null,
    max_attempts integer not null,
    next_attempt_at timestamptz not null,
    last_error text,
    created_at timestamptz not null,
    delivered_at timestamptz
);

create index if not exists delivery_jobs_state_next_attempt_idx
    on delivery_jobs(state, next_attempt_at);
