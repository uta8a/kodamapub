alter table login_sessions
    add column csrf_token text not null default '';
