import {
  type FormEvent,
  type ReactNode,
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";
import { Link, Navigate, Route, Routes, useNavigate, useParams } from "react-router-dom";
import {
  createPost,
  getActor,
  getPost,
  getSession,
  listPosts,
  login,
  logout,
  setCsrfToken,
} from "./api";
import type { ActorProfile, Post } from "./types";

const defaultUsername = import.meta.env.VITE_DEFAULT_USERNAME ?? "alice";

type SessionStatus = "loading" | "anonymous" | "authenticated";

const visibilityLabels: Record<Post["visibility"], string> = {
  Public: "公開",
  Unlisted: "限定公開",
  Followers: "フォロワー",
  Direct: "ダイレクト",
};

const contentFormatLabels: Record<Post["content_format"], string> = {
  Plaintext: "プレーンテキスト",
  Markdown: "マークダウン",
};

type SessionContextValue = {
  actor: ActorProfile | null;
  status: SessionStatus;
  login: (input: { username: string; password: string }) => Promise<void>;
  logout: () => Promise<void>;
};

const SessionContext = createContext<SessionContextValue | null>(null);

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

function formatVisibility(value: Post["visibility"]): string {
  return visibilityLabels[value];
}

function formatContentFormat(value: Post["content_format"]): string {
  return contentFormatLabels[value];
}

function useSession() {
  const value = useContext(SessionContext);
  if (!value) {
    throw new Error("SessionContext is not available");
  }

  return value;
}

function SessionProvider({ children }: { children: ReactNode }) {
  const [actor, setActor] = useState<ActorProfile | null>(null);
  const [status, setStatus] = useState<SessionStatus>("loading");

  useEffect(() => {
    let cancelled = false;

    async function load() {
      try {
        const current = await getSession();
        if (!cancelled) {
          setActor(current.actor);
          setCsrfToken(current.csrf_token);
          setStatus("authenticated");
        }
      } catch {
        if (!cancelled) {
          setActor(null);
          setCsrfToken(null);
          setStatus("anonymous");
        }
      }
    }

    void load();

    return () => {
      cancelled = true;
    };
  }, []);

  async function signIn(input: { username: string; password: string }) {
    const current = await login(input);
    setActor(current.actor);
    setCsrfToken(current.csrf_token);
    setStatus("authenticated");
  }

  async function signOut() {
    await logout();
    setActor(null);
    setCsrfToken(null);
    setStatus("anonymous");
  }

  return (
    <SessionContext.Provider value={{ actor, status, login: signIn, logout: signOut }}>
      {children}
    </SessionContext.Provider>
  );
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
  const navigate = useNavigate();
  const { actor, status, logout: signOut } = useSession();
  const homeLink = actor ? "/home" : "/login";

  async function handleSignOut() {
    await signOut();
    navigate("/login");
  }

  return (
    <div className="app-shell">
      <header className="topbar">
        <Link className="brand" to={homeLink}>
          <span className="brand-mark">k</span>
          <span>
            <strong>kodamapub</strong>
            <small>静かな ActivityPub フロントエンド</small>
            <span className="brand-chip">local pub / notes</span>
          </span>
        </Link>

        <div className="topbar-copy">
          <p>{title}</p>
          <span>{subtitle}</span>
        </div>

        <div className="topbar-actions">
          {status === "loading" ? (
            <div className="session-pill">
              <span>セッション</span>
              <strong>確認中</strong>
            </div>
          ) : actor ? (
            <>
              <div className="session-pill">
                <span>ログイン中</span>
                <strong>@{actor.username}</strong>
              </div>
              <Link className="secondary-button" to={profilePath(actor.username)}>
                プロフィール
              </Link>
              <button className="secondary-button" type="button" onClick={() => void handleSignOut()}>
                ログアウト
              </button>
            </>
          ) : (
            <Link className="secondary-button" to="/login">
              ログイン
            </Link>
          )}
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
        <span>{formatVisibility(post.visibility)}</span>
        <span>{formatContentFormat(post.content_format)}</span>
        <time dateTime={post.created_at}>{formatDate(post.created_at)}</time>
      </div>
      <div className="post-body" dangerouslySetInnerHTML={body} />
      <div className="post-footer">
        <Link to={postPath(username, post.id)}>開く</Link>
        <span className="muted">{post.url}</span>
      </div>
    </article>
  );
}

function Composer({ username, onCreated }: { username: string; onCreated: (post: Post) => void }) {
  const navigate = useNavigate();
  const [contentSource, setContentSource] = useState("こんにちは。");
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
      setError(cause instanceof Error ? cause.message : "投稿を作成できませんでした。");
    } finally {
      setIsSaving(false);
    }
  }

  return (
    <section className="panel composer-panel">
      <div className="panel-header">
        <h2>投稿</h2>
        <span>@{username} で投稿</span>
      </div>

      <form className="composer" onSubmit={submit}>
        <label>
          <span>本文</span>
          <textarea
            value={contentSource}
            onChange={(event) => setContentSource(event.target.value)}
            rows={7}
            placeholder="短く、気軽に書く。"
          />
        </label>

        <div className="composer-row">
          <label>
            <span>形式</span>
            <select
              value={contentFormat}
              onChange={(event) => setContentFormat(event.target.value as Post["content_format"])}
            >
              <option value="Plaintext">プレーンテキスト</option>
              <option value="Markdown">マークダウン</option>
            </select>
          </label>

          <label>
            <span>公開範囲</span>
            <select
              value={visibility}
              onChange={(event) => setVisibility(event.target.value as Post["visibility"])}
            >
              <option value="Public">公開</option>
              <option value="Unlisted">限定公開</option>
              <option value="Followers">フォロワー</option>
              <option value="Direct">ダイレクト</option>
            </select>
          </label>
        </div>

        <label>
          <span>返信先</span>
          <input
            value={replyTo}
            onChange={(event) => setReplyTo(event.target.value)}
            placeholder="必要なら投稿 ID"
          />
        </label>

        <button type="submit" disabled={isSaving}>
          {isSaving ? "送信中..." : "投稿する"}
        </button>

        {error ? <p className="error">{error}</p> : null}
      </form>
    </section>
  );
}

function normalizeHandle(handle: string | undefined): string {
  return handle?.startsWith("@") ? handle.slice(1).trim() : handle?.trim() ?? "";
}

function LoadingPanel({ title, subtitle }: { title: string; subtitle: string }) {
  return (
    <AppShell title={title} subtitle={subtitle}>
      <section className="panel wide-panel">
        <div className="panel-header">
          <h2>読み込み</h2>
          <span>少し待ってください</span>
        </div>
        <p className="summary">セッションを確認し、必要な画面を開いています。</p>
      </section>
    </AppShell>
  );
}

function TimelinePage({
  username,
  title,
  subtitle,
  composerUsername,
}: {
  username: string;
  title: string;
  subtitle: string;
  composerUsername?: string;
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
          setError(cause instanceof Error ? cause.message : "タイムラインを読み込めませんでした。");
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
      setError(cause instanceof Error ? cause.message : "追加の投稿を読み込めませんでした。");
    } finally {
      setIsLoadingMore(false);
    }
  }

  return (
    <AppShell title={title} subtitle={subtitle}>
      <section className="panel profile-panel">
        <div className="panel-header">
          <h2>プロフィール</h2>
          <span>{actor?.actor_url ?? "読み込み中..."}</span>
        </div>

        {error ? (
          <p className="error">{error}</p>
        ) : actor ? (
          <>
            <h1>{actor.display_name}</h1>
            <p className="handle">@{actor.username}</p>
            <p className="summary">{actor.summary ?? "まだ説明文はありません。"}</p>
            <dl className="profile-grid">
              <div>
                <dt>アクター</dt>
                <dd>
                  <a href={actor.actor_url}>{actor.actor_url}</a>
                </dd>
              </div>
              <div>
                <dt>受信箱</dt>
                <dd>{actor.inbox_url ?? "未設定"}</dd>
              </div>
              <div>
                <dt>送信箱</dt>
                <dd>{actor.outbox_url ?? "未設定"}</dd>
              </div>
            </dl>
          </>
        ) : (
          <p className="muted">アクターを読み込み中...</p>
        )}
      </section>

      {composerUsername ? (
        <Composer
          username={composerUsername}
          onCreated={(post) => {
            setPosts((current) => [post, ...current]);
          }}
        />
      ) : null}

      <section className="panel feed-panel">
        <div className="panel-header">
          <h2>投稿一覧</h2>
          <span>{posts.length} 件</span>
        </div>

        <div className="feed-list">
          {posts.length === 0 ? (
            <p className="muted">まだ投稿がありません。</p>
          ) : (
            posts.map((post) => <FeedCard key={post.id} post={post} username={username} />)
          )}
        </div>

        {nextCursor ? (
          <div className="feed-actions">
            <button type="button" onClick={() => void loadMore()} disabled={isLoadingMore}>
              {isLoadingMore ? "読み込み中..." : "もっと読む"}
            </button>
          </div>
        ) : null}
      </section>
    </AppShell>
  );
}

function HomePage() {
  const { actor, status } = useSession();

  if (status === "loading") {
    return <LoadingPanel title="ホーム" subtitle="セッションを確認しています。" />;
  }

  if (!actor) {
    return <Navigate replace to="/login" />;
  }

  return (
    <TimelinePage
      username={actor.username}
      title="ホーム"
      subtitle={`@${actor.username} に見える投稿を並べています。`}
      composerUsername={actor.username}
    />
  );
}

function UserPage() {
  const { handle } = useParams();
  const username = normalizeHandle(handle);

  if (!username) {
    return <Navigate replace to="/login" />;
  }

  return (
    <TimelinePage
      username={username}
      title={`@${username} のプロフィール`}
      subtitle="プロフィールと投稿を表示します。"
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
          setError(cause instanceof Error ? cause.message : "投稿を読み込めませんでした。");
        }
      }
    }

    void load();

    return () => {
      cancelled = true;
    };
  }, [postId]);

  return (
    <AppShell title="投稿" subtitle={`@${username} の投稿詳細です。`}>
      <section className="panel wide-panel">
        <div className="panel-header">
          <h2>投稿</h2>
          <span>{postId}</span>
        </div>

        {error ? (
          <p className="error">{error}</p>
        ) : post ? (
          <FeedCard post={post} username={username} />
        ) : (
          <p className="muted">投稿を読み込み中...</p>
        )}
      </section>
    </AppShell>
  );
}

function LoginPage() {
  const navigate = useNavigate();
  const { actor, status, login: signIn } = useSession();
  const [username, setUsername] = useState(defaultUsername);
  const [password, setPassword] = useState("");
  const [isSigningIn, setIsSigningIn] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setIsSigningIn(true);
    setError(null);

    try {
      await signIn({
        username: normalizeHandle(username),
        password,
      });
      navigate("/home");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "ログインできませんでした。");
    } finally {
      setIsSigningIn(false);
    }
  }

  if (status === "loading") {
    return <LoadingPanel title="ログイン" subtitle="セッションを確認しています。" />;
  }

  if (actor) {
    return <Navigate replace to="/home" />;
  }

  return (
    <AppShell
      title="ログイン"
      subtitle="ローカルユーザーで続行します。"
    >
      <section className="panel login-panel">
        <div className="panel-header">
          <h2>ログイン</h2>
          <span>ローカルセッション</span>
        </div>

        <form className="login-form" onSubmit={submit}>
          <label>
            <span>ユーザー名</span>
            <input
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              placeholder={defaultUsername}
              autoComplete="username"
              spellCheck={false}
            />
          </label>

          <label>
            <span>パスワード</span>
            <input
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              type="password"
              autoComplete="current-password"
            />
          </label>

          <button type="submit" disabled={isSigningIn}>
            {isSigningIn ? "確認中..." : "続行"}
          </button>

          {error ? <p className="error">{error}</p> : null}
        </form>
      </section>

      <section className="panel login-aside">
        <div className="panel-header">
          <h2>この画面でできること</h2>
          <span>ローカルで完結</span>
        </div>

        <p className="summary">
          ユーザー名とパスワードをサーバーで確認し、セッション cookie を保存します。
          ユーザーの作成や更新は CLI 側で行います。
        </p>

        <ul className="feature-list">
          <li>パスワードは CLI でユーザーを作成・更新するときに設定します。</li>
          <li>ブラウザに残るのはセッション cookie だけです。</li>
          <li>ホームの投稿欄は、ログイン中のローカルアクターを使います。</li>
        </ul>

        <div className="login-note">
          <span>推奨アカウント</span>
          <strong>@{defaultUsername}</strong>
        </div>
      </section>
    </AppShell>
  );
}

function NotFoundPage() {
  return (
    <AppShell title="見つかりません" subtitle="指定された画面はありません。">
      <section className="panel wide-panel">
        <div className="panel-header">
          <h2>404</h2>
          <span>見つかりません</span>
        </div>

        <h1 className="hero-title">そのページはありません。</h1>
        <p className="summary">
          ホームへ戻るか、プロフィールを開いてください。
        </p>

        <div className="button-row">
          <Link className="secondary-button" to="/home">
            ホームへ
          </Link>
          <Link className="secondary-button" to="/login">
            ログイン
          </Link>
        </div>
      </section>
    </AppShell>
  );
}

function RootRedirect() {
  const { actor, status } = useSession();

  if (status === "loading") {
    return <LoadingPanel title="kodamapub" subtitle="セッションを確認しています。" />;
  }

  return <Navigate replace to={actor ? "/home" : "/login"} />;
}

export function App() {
  return (
    <SessionProvider>
      <Routes>
        <Route path="/" element={<RootRedirect />} />
        <Route path="/login" element={<LoginPage />} />
        <Route path="/home" element={<HomePage />} />
        <Route path="/:handle/:postId" element={<PostPage />} />
        <Route path="/:handle" element={<UserPage />} />
        <Route path="*" element={<NotFoundPage />} />
      </Routes>
    </SessionProvider>
  );
}
