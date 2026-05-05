import type { ActorProfile, CreatePostInput, Post, PostPage } from "./types";

export type LoginInput = {
  username: string;
  password: string;
};

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    credentials: "include",
    headers: {
      "content-type": "application/json",
      accept: "application/json",
      ...(init?.headers ?? {}),
    },
    ...init,
  });

  if (!response.ok) {
    throw new Error(`request failed: ${response.status} ${response.statusText}`);
  }

  return response.json() as Promise<T>;
}

export async function getActor(username: string): Promise<ActorProfile> {
  return request(`/api/users/${encodeURIComponent(username)}`);
}

export async function listPosts(
  username: string,
  options?: { limit?: number; before?: string | null },
): Promise<PostPage> {
  const params = new URLSearchParams();
  params.set("limit", String(options?.limit ?? 20));
  if (options?.before) {
    params.set("before", options.before);
  }

  return request(`/api/users/${encodeURIComponent(username)}/posts?${params.toString()}`);
}

export async function getPost(postId: string): Promise<Post> {
  return request(`/api/posts/${encodeURIComponent(postId)}`);
}

export async function createPost(username: string, input: CreatePostInput): Promise<Post> {
  return request(`/api/users/${encodeURIComponent(username)}/posts`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function getSession(): Promise<ActorProfile> {
  return request("/api/session");
}

export async function login(input: LoginInput): Promise<ActorProfile> {
  return request("/api/login", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function logout(): Promise<void> {
  const response = await fetch("/api/logout", {
    method: "POST",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(`request failed: ${response.status} ${response.statusText}`);
  }
}
