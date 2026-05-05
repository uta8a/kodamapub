export type ActorProfile = {
  id: string;
  username: string;
  display_name: string;
  summary: string | null;
  actor_url: string;
  inbox_url: string | null;
  outbox_url: string | null;
};

export type Post = {
  id: string;
  actor_id: string;
  url: string;
  content_source: string;
  content_format: 'Plaintext' | 'Markdown';
  content_html: string;
  visibility: 'Public' | 'Unlisted' | 'Followers' | 'Direct';
  in_reply_to: string | null;
  created_at: string;
};

export type CreatePostInput = {
  content_source: string;
  content_format: Post['content_format'];
  visibility: Post['visibility'];
  in_reply_to: string | null;
};
