import { Bell, Bot, Clock3, Files, FolderOpen, HardDrive, History, LockKeyhole, Share2, TerminalSquare, Trash2, UserRoundCheck } from "lucide-react";
import type { AppConfig } from "./app-config.types";
export const appConfig: AppConfig = {
  appId: "briefcase", name: "Briefcase", brandPrefix: "Silicon",
  tagline: "A place for everything you make.",
  description: "Your files, thoughtfully kept. Briefcase gives every Carbon and Silicon a personal drive, precise sharing, file previews and a history you can return to.",
  mark: { paths: ["M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2", "M4 6h16a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1z", "M3 12h18", "M10 11v3h4v-3"] },
  cli: { command: "briefcase" }, home: "/files",
  nav: [
    { href: "/files", label: "My files", icon: FolderOpen, keywords: ["drive", "upload"] },
    { href: "/shared", label: "Shared with me", icon: Share2 },
    { href: "/recent", label: "Recent", icon: Clock3 },
    { href: "/silicons", label: "My Silicons", icon: Bot },
    { href: "/bin", label: "Bin", icon: Trash2 },
    { href: "/inbox", label: "Notifications", icon: Bell },
    { href: "/settings", label: "Settings", icon: HardDrive },
  ],
  links: { docs: "https://docs.briefcase.teamofsilicons.com", store: "https://apps.teamofsilicons.com/apps/briefcase", source: "https://github.com/teamofsilicons/silicon-briefcase" },
  signIn: { scopes: ["email"] },
  api: { forwardHeaders: [], exposeHeaders: ["x-api-version", "x-operation-version"], telemetryOptOut: "x-briefcase-telemetry" },
  landing: {
    headline: "Everything you make,\nin its place.",
    lede: "A quiet home for your files. Keep a project close, share exactly what you mean to, and pick up where you left off — together with your Silicons.",
    forCarbons: [
      { icon: Files, title: "Room for your work", text: "Documents, images, recordings and everything in between. Find a file, preview it, and carry on." },
      { icon: Share2, title: "Sharing with intention", text: "Invite a Carbon or Silicon by its id. Choose their access, set a deadline, or create a link." },
      { icon: History, title: "A way back", text: "Return to an earlier version. Restore an accidental deletion from your bin for 45 days." },
      { icon: LockKeyhole, title: "Your drive, your choice", text: "Keep files in Briefcase or connect your own S3 bucket. Your files stay yours." },
    ],
    forSilicons: [
      { icon: TerminalSquare, title: "Files from the command line", text: "Install with silicon-apps install briefcase. Browse, stream and share through the same service." },
      { icon: Bot, title: "A drive of your own", text: "Sign in with a short-lived token from Silicon Accounts. No browser, no shared credentials." },
      { icon: UserRoundCheck, title: "Working together", text: "Your custodian can look after your files. You choose which other accounts can send you shares." },
    ],
  },
};
export type { AppConfig, NavItem } from "./app-config.types";
