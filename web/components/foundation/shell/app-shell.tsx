"use client";

/**
 * The signed-in workspace's frame, the one Silicon Accounts has (silicon-accounts web/components/foundation/shell/
 * account-shell.tsx), so the two read as one product: a top bar with the brand (home), search (the ⌘K palette) and the
 * account menu, then the section list (sidebar.tsx) beside a white page box from 900px, in a bottom sheet opened from the
 * top bar below that. Also the ⌘K palette, number-key shortcuts and page transitions along the sections.
 *
 * The workspace layout (a Server Component) has already checked the session, so the shell renders signed in from its
 * first frame: no loading flash, no client-side "who am I" request.
 */
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { ViewTransition, useCallback, useEffect, useMemo, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { ArrowLeftRight, BookOpen, CircleUserRound, LogOut, Menu, Search } from "lucide-react";
import { BottomSheet } from "@teamofsilicons/silicon-ui/bottom-sheet";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { UserMenu, type UserMenuUser } from "@teamofsilicons/silicon-ui/user-menu";
import { SkipLink } from "@/components/foundation/layout/layout";
import type { SessionAccount } from "@/lib/account";
import { appConfig } from "@/lib/app.config";
import { useSignOut } from "@/lib/client/session";
import { openCommandPalette, toggleCommandPalette, useCommandPaletteOpen, useRegisterCommands } from "@/lib/commands";
import { navigationType, navItemFor } from "@/lib/navigation";
import { notifyError } from "@/lib/notify";
import { BrandWords } from "./brand-mark";
import { CommandMenu } from "./command-menu";
import { Sidebar, SWITCH_ACCOUNT } from "./sidebar";
import { SIDEBAR_STEPS, SidebarResize, useSidebarStep } from "./sidebar-resize";
import styles from "./shell.module.css";

const subscribeNothing = () => () => undefined;

/** Under 900px the section list moves from beside the page into a bottom sheet. */
const NARROW = "(max-width: 899px)";
const subscribeNarrow = (change: () => void) => {
  const media = window.matchMedia(NARROW);
  media.addEventListener("change", change);
  return () => media.removeEventListener("change", change);
};
const readNarrow = () => window.matchMedia(NARROW).matches;

function detectApple(): boolean {
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  return /mac|iphone|ipad|ipod/i.test(nav.userAgentData?.platform ?? nav.platform ?? "");
}

/** True when the event target is a text field, so global shortcuts never steal keys from typing. */
function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target.tagName === "TEXTAREA" || target.tagName === "SELECT") return true;
  if (target.tagName !== "INPUT") return false;
  return !["button", "checkbox", "radio", "submit", "reset", "range", "color", "file"].includes((target as HTMLInputElement).type);
}

/** The sections' number keys, 1 to 9 down the sidebar. */
const shortcutFor = (index: number) => (index < 9 ? String(index + 1) : undefined);

const PAGE_CLASSES = { "nav-forward": "page-forward", "nav-back": "page-back", "nav-fade": "page-fade", default: "page-fade" };

export interface AppShellProps {
  account: SessionAccount;
  /** Silicon Accounts' public origin: "Your Account" opens it. */
  accountsUrl: string;
  children: ReactNode;
}

export function AppShell({ account, accountsUrl, children }: AppShellProps) {
  const pathname = usePathname();
  const router = useRouter();
  const { signOut } = useSignOut();
  const paletteOpen = useCommandPaletteOpen();
  const isApple = useSyncExternalStore(subscribeNothing, detectApple, () => false);
  const narrow = useSyncExternalStore(subscribeNarrow, readNarrow, () => false);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [sidebarStep, setSidebarStep] = useSidebarStep();
  const section = navItemFor(pathname);

  /** Every navigation the shell starts moves inside a page transition whose direction follows the sidebar. */
  const go = useCallback((href: string) => {
    if (href === window.location.pathname) return;
    router.push(href, { transitionTypes: [navigationType(window.location.pathname, href)] });
  }, [router]);

  // From 900px the page box is the scroller, not the window (shell.module.css), so a new page starts at its top.
  useEffect(() => {
    document.getElementById("main")?.scrollTo({ top: 0 });
  }, [pathname]);

  const doSignOut = useCallback(async () => {
    try {
      await signOut();
    } catch (failure) {
      notifyError(failure, "Could not sign out");
    }
  }, [signOut]);

  const openAccount = useCallback(() => void window.open(accountsUrl, "_blank", "noopener"), [accountsUrl]);

  useRegisterCommands(() => [
    ...appConfig.nav.map((item, index) => {
      const Icon = item.icon;
      return { id: `go.${item.href}`, label: item.label, group: "Go to", shortcut: shortcutFor(index), icon: <Icon size={16} strokeWidth={2} />, keywords: item.keywords, run: () => go(item.href) };
    }),
    { id: "go.docs", label: `${appConfig.name} Docs`, description: "Opens in a new tab", group: "Other sites", icon: <BookOpen size={16} strokeWidth={2} />, keywords: ["docs", "help", "guide"], run: () => void window.open(appConfig.links.docs, "_blank", "noopener") },
    { id: "go.account", label: "Your Account", description: "Silicon Accounts, in a new tab", group: "Other sites", icon: <CircleUserRound size={16} strokeWidth={2} />, keywords: ["account", "profile", "photo"], run: openAccount },
    { id: "account.switch", label: "Switch Account", description: "Your personal or a work account", group: "Account", icon: <ArrowLeftRight size={16} strokeWidth={2} />, keywords: ["work", "personal", "organization", "workspace"], run: () => window.location.assign(SWITCH_ACCOUNT) },
    { id: "account.signout", label: "Sign Out", description: `Sign this browser out of ${appConfig.name}`, group: "Account", icon: <LogOut size={16} strokeWidth={2} />, keywords: ["logout", "log out", "sign out"], run: () => void doSignOut() },
  ], [go, openAccount, doSignOut]);

  // ⌘K toggles the palette; 1 to 9 jump to sections (never while typing or with a layer open).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        toggleCommandPalette();
        return;
      }
      if (event.metaKey || event.ctrlKey || event.altKey || event.repeat || paletteOpen || isTypingTarget(event.target)) return;
      // Any open layer (Radix dialogs, drawers and sheets carry no aria-modal), menu or popup list keeps the keys.
      const layers = document.querySelectorAll("[role='dialog'], [role='alertdialog'], [role='menu'], [role='listbox']");
      if ([...layers].some(layer => layer.getClientRects().length > 0)) return;
      const index = appConfig.nav.findIndex((_, position) => shortcutFor(position) === event.key);
      if (index < 0) return;
      event.preventDefault();
      go(appConfig.nav[index].href);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [go, paletteOpen]);

  const user = useMemo<UserMenuUser>(() => ({ name: account.display_name, email: account.id, avatarSrc: account.pfp_url ?? undefined }), [account]);

  return (
    <div className={styles.shell}>
      <SkipLink href="#main" />
      <header className={styles.top}>
        <Link href={appConfig.home} aria-label={`Team of Silicons ${appConfig.name}, home`} data-sq="surface" className={styles.brand} transitionTypes={[navigationType(pathname, appConfig.home)]}>
          <BrandWords />
        </Link>
        {/* Keyed by the layout: Silicon UI's Button measures its label once and keeps that width inline, so after a resize
            from wide to narrow it stayed 400px and covered the brand. A new Button measures again. */}
        <Button key={narrow ? "narrow" : "wide"} variant="soft" size="sm" className={styles.search} onClick={openCommandPalette} aria-keyshortcuts={isApple ? "Meta+K" : "Control+K"} aria-label={narrow ? `Search ${appConfig.name}` : undefined}>
          <Search size={16} strokeWidth={2.1} aria-hidden="true" />
          <span className={styles.searchLabel}>Search</span>
          <kbd data-sq="surface" className={styles.keys}>{isApple ? "⌘K" : "Ctrl K"}</kbd>
        </Button>
        <span className={styles.topEnd}>
          <UserMenu
            className={styles.account}
            user={user}
            showTheme={false}
            onSignOut={doSignOut}
            align="end"
            items={[
              { label: "Your Account", icon: <CircleUserRound size={16} strokeWidth={2} />, onSelect: openAccount },
              { label: "Switch Account", icon: <ArrowLeftRight size={16} strokeWidth={2} />, onSelect: () => window.location.assign(SWITCH_ACCOUNT) },
            ]}
          />
          {narrow ? (
            <BottomSheet
              title={appConfig.name}
              open={sheetOpen}
              onOpenChange={setSheetOpen}
              trigger={<Button variant="ghost" size="sm" aria-label={`${appConfig.name} sections`}><Menu size={18} strokeWidth={2} aria-hidden="true" /></Button>}
            >
              <Sidebar accountsUrl={accountsUrl} onNavigate={() => setSheetOpen(false)} />
            </BottomSheet>
          ) : null}
        </span>
      </header>
      <div className={styles.body} style={{ "--sidebar-width": `${SIDEBAR_STEPS[sidebarStep]}px` } as CSSProperties}>
        {narrow ? null : (
          <>
            <aside id="briefcase-sidebar" className={styles.aside}>
              <Sidebar accountsUrl={accountsUrl} />
            </aside>
            <SidebarResize step={sidebarStep} onStep={setSidebarStep} controls="briefcase-sidebar" />
          </>
        )}
        <main id="main" data-sq-native="" className={styles.main} tabIndex={-1}>
          <ViewTransition key={section?.href ?? pathname} enter={PAGE_CLASSES} exit={PAGE_CLASSES} default="none">
            <div className={styles.page}>{children}</div>
          </ViewTransition>
        </main>
      </div>
      <CommandMenu />
    </div>
  );
}
