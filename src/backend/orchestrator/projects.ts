import { realpath, stat } from "node:fs/promises"
import { basename } from "node:path"
import { RpcError, invalidParams } from "../errors.ts"
import {
  addEvent,
  getProject,
  listProjects,
  upsertProject,
  type ProjectRow,
} from "../db/repositories.ts"

export async function registerProjectHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const inputPath = typeof params.path === "string" ? params.path.trim() : ""
  if (!inputPath) throw invalidParams("path is required")

  let canonical: string
  try {
    canonical = await realpath(inputPath)
    const info = await stat(canonical)
    if (!info.isDirectory()) throw new Error("not a directory")
  } catch {
    throw invalidParams(`project path is not a directory: ${inputPath}`)
  }

  let name = basename(canonical)
  if (params.name !== undefined) {
    if (typeof params.name !== "string" || params.name.trim() === "") {
      throw invalidParams("--name cannot be empty")
    }
    name = params.name.trim()
  }

  let botRoles = ["ceo", "git", "ops", "audit"]
  if (params.botRoles !== undefined) {
    const value = params.botRoles
    if (
      !Array.isArray(value) ||
      value.some((role) => typeof role !== "string" || role.trim() === "")
    ) {
      throw invalidParams("botRoles must be an array of strings")
    }
    botRoles = (value as string[]).map((role) => role.trim())
  }

  const { project, created } = upsertProject({ path: canonical, name, botRoles })
  if (created) {
    addEvent("project.registered", `Project ${project.name} registered`, { projectId: project.projectId, path: canonical })
  }

  return { projectId: project.projectId, path: project.path, name: project.name, botRoles: project.botRoles, created }
}

export async function listProjectsHandler(): Promise<{ items: ProjectRow[] }> {
  return { items: listProjects() }
}

export async function getProjectHandler(params: Record<string, unknown>): Promise<ProjectRow> {
  const ref = typeof params.projectId === "string" ? params.projectId.trim() : ""
  if (!ref) throw invalidParams("projectId is required")
  const project = getProject(ref)
  if (!project) throw new RpcError(-32602, `project not found: ${ref}`)
  return project
}
