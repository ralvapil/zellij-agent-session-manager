import fs from "node:fs"
import os from "node:os"
import path from "node:path"

const stateDir = process.env.XDG_STATE_HOME
  ? path.join(process.env.XDG_STATE_HOME, "opencode-zellij")
  : path.join(os.homedir(), ".local", "state", "opencode-zellij")
const alertsFile = path.join(stateDir, "alerts.json")
const waitingEventTypes = new Set(["permission.asked", "question.asked"])

export const ZellijSidebarAlerts = async ({ $, directory }) => {
  return {
    event: async ({ event }) => {
      if (event.type !== "session.idle" && !waitingEventTypes.has(event.type)) return
      if (!process.env.ZELLIJ) return

      const paneId = normalizePaneId(process.env.ZELLIJ_PANE_ID)
      if (!paneId) return

      const kind = waitingEventTypes.has(event.type) ? "waiting" : "done"
      const status = kind === "waiting" ? "waiting" : "idle"
      const message = kind === "waiting" ? "OpenCode needs input" : "OpenCode finished"

      fs.mkdirSync(stateDir, { recursive: true })
      const alerts = readAlerts()
      alerts.agents[paneId] = {
        ...(alerts.agents[paneId] || {}),
        agent: "opencode",
        pane_id: paneId,
        zellij_session: process.env.ZELLIJ_SESSION_NAME || null,
        cwd: directory,
        kind,
        status,
        unread: true,
        message,
        updated_at: Date.now(),
      }
      writeAlerts(alerts)

      const payload = JSON.stringify(alerts.agents[paneId])
      const pipeName = kind === "waiting" ? "opencode.waiting" : "opencode.done"
      await $`sh -lc ${zellijPipeCommand(pipeName, payload)}`
    },
  }
}

function zellijPipeCommand(pipeName, payload) {
  const session = process.env.ZELLIJ_SESSION_NAME
  const sessionArgs = session ? ` --session ${shellQuote(session)}` : ""
  return `zellij${sessionArgs} action pipe --name ${shellQuote(pipeName)} -- ${shellQuote(payload)}`
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`
}

function readAlerts() {
  try {
    const parsed = JSON.parse(fs.readFileSync(alertsFile, "utf8"))
    if (!parsed.agents) parsed.agents = {}
    return parsed
  } catch {
    return { agents: {} }
  }
}

function writeAlerts(alerts) {
  const tmp = `${alertsFile}.${process.pid}.tmp`
  fs.writeFileSync(tmp, `${JSON.stringify(alerts, null, 2)}\n`)
  fs.renameSync(tmp, alertsFile)
}

function normalizePaneId(value) {
  if (!value) return null
  if (/^(terminal|plugin)_\d+$/.test(value)) return value
  if (/^\d+$/.test(value)) return `terminal_${value}`
  return value
}
