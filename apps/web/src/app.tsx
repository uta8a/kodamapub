import { type FormEvent, type ReactNode, useEffect, useMemo, useState } from "react";
import { Link, Navigate, Route, Routes, useNavigate, useParams } from "react-router-dom";
import { createPost, getActor, getPost, listPosts } from "./api";
import type { ActorProfile, Post } from "./types";

const defaultUsername = import.meta.env.VITE_DEFAULT_USERNAME ?? "alice";

function profilePath(username: string): string {
  return `/@${username}`;
}

function postPath(username: string, postId: string): string {
  return `${profilePath(username)}/${postId}`;
}

function formatDate(value: string): string {
  return new Intl.DateTimeFormat("ja-JP", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}

function AppShell({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle: string;
  children: ReactNode;
}) {
  return (
    <div className="app-shell">
      <header className="topbar">
        <Link className="brand" to={`/@${defaultUsername}`}>
          <span className="brand-mark">k</span>
          <span>
            <strong>kodamapub</strong>
            <small>ActivityPub front-end</small>
          </span>
        </Link>

        <div className="topbar-copy">
          <p>{title}</p>
          <span>{subtitle}</span>
        </div>
      </header>

      <main className="page-grid">{children}</main>
    </div>
  );
}

function FeedCard({ post, username }: { post: Post; username: string }) {
  const body = useMemo(() => ({ __html: post.content_html }), [post.content_html]);

  return (
    <article className="post-card">
      <div className="post-meta">
        <span>{post.visibility}</span>
        <span>{post.content_format}</span>
        <time dateTime={post.created_at}>{formatDate(post.created_at)}</time>
      </div>
      <div className="post-body" dangerouslySetInnerHTML={body} />
      <div className="post-footer">
        <Link to={postPath(username, post.id)}>Open post</Link>
        <span className="muted">{post.url}</span>
      </div>
    </article>
  );
}

function Composer({ username, onCreated }: { username: string; onCreated: (post: Post) => void }) {
  const navigate = useNavigate();
  const [contentSource, setContentSource] = useState("Hello, world.");
  const [visibility, setVisibility] = useState<Post["visibility"]>("Public");
  const [contentFormat, setContentFormat] = useState<Post["content_format"]>("Plaintext");
  const [replyTo, setReplyTo] = useState("");
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setIsSaving(true);
    setError(null);

    try {
      const post = await createPost(username, {
        content_source: contentSource,
        content_format: contentFormat,
        visibility,
        in_reply_to: replyTo.trim() ? replyTo.trim() : null,
      });
      onCreated(post);
      setContentSource("");
      setReplyTo("");
      navigate(postPath(username, post.id));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "failed to create post");
    } finally {
      setIsSaving(false);
    }
  }

  return (
    <section className="panel composer-panel">
      <div className="panel-header">
        <h2>Compose</h2>
        <span>posting as @{username}</span>
      </div>

      <form className="composer" onSubmit={submit}>
        <label>
          <span>Content</span>
          <textarea
            value={contentSource}
            onChange={(event) => setContentSource(event.target.value)}
            rows={7}
            placeholder="Write something compact."
          />
        </label>

        <div className="composer-row">
          <label>
            <span>Format</span>
            <select
              value={contentFormat}
              onChange={(event) => setContentFormat(event.target.value as Post["content_format"])}
            >
              <option value="Plaintext">Plaintext</option>
              <option value="Markdown">Markdown</option>
            </select>
          </label>

          <label>
            <span>Visibility</span>
            <select
              value={visibility}
              onChange={(event) => setVisibility(event.target.value as Post["visibility"])}
            >
              <option value="Public">Public</option>
              <option value="Unlisted">Unlisted</option>
              <option value="Followers">Followers</option>
              <option value="Direct">Direct</option>
            </select>
          </label>
        </div>

        <label>
          <span>Reply to</span>
          <input
            value={replyTo}
            onChange={(event) => setReplyTo(event.target.value)}
            placeholder="Optional post UUID"
          />
        </label>

        <button type="submit" disabled={isSaving}>
          {isSaving ? "Publishing..." : "Publish"}
        </button>

        {error ? <p className="error">{error}</p> : null}
      </form>
    </section>
  );
}

function normalizeHandle(handle: string | undefined): string {
  return handle?.startsWith("@") ? handle.slice(1) : (handle ?? defaultUsername);
}

function TimelinePage({
  username,
  title,
  subtitle,
}: {
  username: string;
  title: string;
  subtitle: string;
}) {
  const [actor, setActor] = useState<ActorProfile | null>(null);
  const [posts, setPosts] = useState<Post[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoadingMore, setIsLoadingMore] = useState(false);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      setError(null);
      setActor(null);
      setPosts([]);
      setNextCursor(null);
      try {
        const [actorData, postPage] = await Promise.all([getActor(username), listPosts(username)]);
        if (cancelled) {
          return;
        }
        setActor(actorData);
        setPosts(postPage.posts);
        setNextCursor(postPage.next_cursor);
      } catch (cause) {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : "failed to load timeline");
        }
      }
    }

    void load();

    return () => {
      cancelled = true;
    };
  }, [username]);

  async function loadMore() {
    if (!nextCursor) {
      return;
    }

    setIsLoadingMore(true);
    setError(null);

    try {
      const page = await listPosts(username, { before: nextCursor });
      setPosts((current) => [...current, ...page.posts]);
      setNextCursor(page.next_cursor);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "failed to load more posts");
    } finally {
      setIsLoadingMore(false);
    }
  }

  return (
    <AppShell title={title} subtitle={subtitle}>
      <section className="panel profile-panel">
        <div className="panel-header">
          <h2>Profile</h2>
          <span>{actor?.actor_url ?? "loading..."}</span>
        </div>

        {error ? (
          <p className="error">{error}</p>
        ) : actor ? (
          <>
            <h1>{actor.display_name}</h1>
            <p className="handle">@{actor.username}</p>
            <p className="summary">{actor.summary ?? "No summary yet."}</p>
            <dl className="profile-grid">
              <div>
                <dt>Actor</dt>
                <dd>
                  <a href={actor.actor_url}>{actor.actor_url}</a>
                </dd>
              </div>
              <div>
                <dt>Inbox</dt>
                <dd>{actor.inbox_url ?? "unset"}</dd>
              </div>
              <div>
                <dt>Outbox</dt>
                <dd>{actor.outbox_url ?? "unset"}</dd>
              </div>
            </dl>
          </>
        ) : (
          <p className="muted">Loading actor...</p>
        )}
      </section>

      <Composer
        username={username}
        onCreated={(post) => {
          setPosts((current) => [post, ...current]);
        }}
      />

      <section className="panel feed-panel">
        <div className="panel-header">
          <h2>Posts</h2>
          <span>{posts.length} items</span>
        </div>

        <div className="feed-list">
          {posts.length === 0 ? (
            <p className="muted">No posts yet.</p>
          ) : (
            posts.map((post) => <FeedCard key={post.id} post={post} username={username} />)
          )}
        </div>

        {nextCursor ? (
          <div className="feed-actions">
            <button type="button" onClick={() => void loadMore()} disabled={isLoadingMore}>
              {isLoadingMore ? "Loading..." : "Load more"}
            </button>
          </div>
        ) : null}
      </section>
    </AppShell>
  );
}

function HomePage() {
  return (
    <TimelinePage
      username={defaultUsername}
      title="Home timeline"
      subtitle="Posts visible from the logged-in local user."
    />
  );
}

function UserPage() {
  const { handle } = useParams();
  const username = normalizeHandle(handle);

  return (
    <TimelinePage
      username={username}
      title={`Profile for @${username}`}
      subtitle="Profile and posts for a local actor."
    />
  );
}

function PostPage() {
  const { handle, postId = "" } = useParams();
  const username = normalizeHandle(handle);
  const [post, setPost] = useState<Post | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      setPost(null);
      setError(null);
      try {
        const data = await getPost(postId);
        if (!cancelled) {
          setPost(data);
        }
      } catch (cause) {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : "failed to load post");
        }
      }
    }

    void load();

    return () => {
      cancelled = true;
    };
  }, [postId]);

  return (
    <AppShell
      title="Single post view"
      subtitle={`Post detail for @${username}.`}
    >
      <section className="panel wide-panel">
        <div className="panel-header">
          <h2>Post</h2>
          <span>{postId}</span>
        </div>

        {error ? (
          <p className="error">{error}</p>
        ) : post ? (
          <FeedCard post={post} username={username} />
        ) : (
          <p className="muted">Loading post...</p>
        )}
      </section>
    </AppShell>
  );
}

export function App() {
  return (
    <Routes>
      <Route path="/" element={<Navigate replace to="/home" />} />
      <Route path="/home" element={<HomePage />} />
      <Route path="/:handle/:postId" element={<PostPage />} />
      <Route path="/:handle" element={<UserPage />} />
    </Routes>
  );
}
