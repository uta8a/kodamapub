create table if not exists inbox_dedup (
    activity_id text primary key,
    actor_id uuid not null references actors(id) on delete cascade,
    activity_type text not null,
    received_at timestamptz not null
);

create index if not exists inbox_dedup_actor_id_received_at_idx
    on inbox_dedup(actor_id, received_at desc);
