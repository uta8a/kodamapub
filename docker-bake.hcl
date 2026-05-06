group "default" {
  targets = ["edge", "server", "web", "cli-job"]
}

target "common" {
  context = "."
}

target "edge" {
  inherits  = ["common"]
  dockerfile = "scripts/docker/edge.Dockerfile"
  tags       = ["kodamapub-edge:latest"]
  cache-from = [
    {
      type  = "gha"
      scope = "kodamapub-edge"
    },
  ]
  cache-to = [
    {
      type  = "gha"
      mode  = "max"
      scope = "kodamapub-edge"
    },
  ]
}

target "server" {
  inherits  = ["common"]
  dockerfile = "scripts/docker/server.Dockerfile"
  tags       = ["kodamapub-server:latest"]
  cache-from = [
    {
      type  = "gha"
      scope = "kodamapub-server"
    },
  ]
  cache-to = [
    {
      type  = "gha"
      mode  = "max"
      scope = "kodamapub-server"
    },
  ]
}

target "web" {
  inherits  = ["common"]
  dockerfile = "scripts/docker/web.Dockerfile"
  tags       = ["kodamapub-web:latest"]
  cache-from = [
    {
      type  = "gha"
      scope = "kodamapub-web"
    },
  ]
  cache-to = [
    {
      type  = "gha"
      mode  = "max"
      scope = "kodamapub-web"
    },
  ]
}

target "cli-job" {
  inherits  = ["common"]
  dockerfile = "scripts/docker/cli-job.Dockerfile"
  tags       = ["kodamapub-cli-job:latest"]
  cache-from = [
    {
      type  = "gha"
      scope = "kodamapub-cli-job"
    },
  ]
  cache-to = [
    {
      type  = "gha"
      mode  = "max"
      scope = "kodamapub-cli-job"
    },
  ]
}
