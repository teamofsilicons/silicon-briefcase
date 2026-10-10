# Proposed changes to Briefcase's UNDERSTANDING.md

UNDERSTANDING.md is changed only by Carbons. These are proposals, in the contract's own plain style, for the
Carbon to paste or rewrite. Each stage adds to this file.

## From service part 1 (identity), 2026-10-10

### Replace "How Does login/signup work"

> Signing in and signing up are handled entirely by Silicon Accounts (accounts.teamofsilicons.com). Every
> Carbon and Silicon has one personal account: a permanent uuid (what Briefcase keys everything on) and a public
> id (`c:handle` or `si:handle`) that can change and is only shown. Briefcase has an app id (`briefcase`) and an
> app secret from Silicon Apps, kept on the server only.
>
> Carbons sign in on the website through the Silicon Accounts sign-in pages, and in the CLI with the device flow
> (the CLI shows a code and a link). Silicons never see a page: they get a short-lived token with
> `silicon-accounts login --app briefcase -q` and give it to Briefcase (`briefcase login <slt>`). Either way
> Briefcase receives a Silicon Accounts access token issued to Briefcase and verifies it on every request.
> Use the official Silicon Accounts client everywhere (https://crates.io/crates/silicon-accounts-client).
>
> Silicon Accounts calls the webhook endpoint (`backend.briefcase.teamofsilicons.com/webhook/`) whenever an
> account changes its id, its name or photo, signs out, removes Briefcase's access, is deleted, or (for a
> Silicon) gets a new custodian. When an account signs out or removes Briefcase's access, its earlier tokens
> stop working at once.

### Replace "How organisations are defined"

> There are no organisations. Every account is personal. A Silicon always has exactly one custodian (a Carbon);
> the custodian can see and manage that Silicon's files, but never acts as the Silicon. (The file-system
> sections are proposed with the drive model in a later stage.)

### Replace "How other apps would use Briefcase"

> Other apps act for an account at Briefcase with a Silicon Accounts User verification proof, sent as
> `Authorization: Proof sap_…`. The proof names the account and the scopes the app may use; Briefcase checks it
> with Silicon Accounts on every call and only lets each app use the scopes its operator allowed for that app.
> The scopes are: create a folder (`briefcase.folders.create`), list entries (`briefcase.entries.list`), read a
> file (`briefcase.files.read`), move an app-created entry to the bin (`briefcase.entries.trash`), reserve,
> commit, check or cancel an upload (`briefcase.uploads.*`), share an entry (`briefcase.invitations.create`)
> and turn "anyone with the link can view" on or off (`briefcase.link_access.update`). Everything an app
> creates records which app created it.

### Remove "Testing Environment" (all subsections)

Honeycomb testing worlds are retired with Honeycomb; Briefcase is tested against a local Silicon Accounts stack
instead. Suggested one-line replacement: "Development and testing use a local Silicon Accounts stack; there is
no separate test environment inside Briefcase."

### CLI section

Replace `iam --json` with `accounts --json` (the CLI stage keeps a hidden `iam --json` alias for one release),
and "short lived tokens that the user can generate from the official iam cli" with "from `silicon-accounts
login --app briefcase -q`".

## From service part 2 (drives and permissions), 2026-10-10

These replace the organisation-based file-system sections. The URL sections are proposed with part 3.

### Glossary: replace `Org`

> `Drive` - Every Carbon and every Silicon has exactly one drive: its own space in Briefcase. Everything you
> upload lives in some drive, and every drive belongs to one account.

### Replace the opening of "How it works"

> Every Carbon and Silicon has its own drive, under which we manage all of its files. For each file there are
> CRUD operations and rules for who can perform each of them.
>
> Each drive has an upload limit of 100gb per day (resets at 12:00am utc) and a maximum total storage of 1 peta
> byte. Both are configurable per drive by a variable in the database. An upload counts against the drive the file
> lands in, whoever uploads it.
>
> For every upload it should be possible to name the exact folder: by its id, or by its path inside a drive (your
> own by default). Without a folder the file goes at the top of the drive. It shouldn't be possible to save files
> directly inside `apps/`, or anywhere you don't have access to.
>
> There should also be endpoints that show how much a drive is currently using, not in percent but the actual
> space, to the drive's owner (and, for a Silicon's drive, its custodian).

### Replace "How to store data" (keep the bucket steps)

> The owner of a drive can choose to use their own S3 bucket instead of ours; for a Silicon's drive its custodian
> can too. The setup steps are the same (demo upload, every CRUD operation, delete). Briefcase gives each drive a
> fixed External ID to put in the role's trust policy, and keeps using it.
>
> Frontend note: show "configure your S3 bucket" only inside that drive's storage settings.

### Replace "Special folder types" and "Folder Structure"

> When you open Briefcase you see:
> - **My files**: your own drive. You can do everything in it.
> - **Shared with me**: everything other Carbons and Silicons shared with you, grouped by whose drive it is in.
> - for a Carbon, **one entry per Silicon you look after**: you can see and manage everything in your Silicons'
>   drives, but you never act as the Silicon — whatever you do there is recorded as you.
>
> Every drive has an `apps/` folder at the top with one folder per app that stored something for you
> (`apps/waveform/`, …). `apps` is kept for this; nobody can create, rename or share these folders themselves.
>
> In someone else's drive you only see what was shared with you and what you created there. A folder in someone
> else's drive leads to what was shared with you inside it, and shows only its name.
>
> Frontend note: on every folder in someone else's drive add a small info box at the bottom - You might not be
> seeing all the contents of this folder. This is a permission based folder.

### Replace "Folder creation"

> You can create folders at the top of your own drive (and of your Silicons' drives), and inside any folder where
> you have write access. Ones with only read access can't create folders. While creating a folder you can invite
> Carbons and Silicons by their id.

### Replace "Permissions" and "Org_admins and Org_Owners"

> Similar to how a linux filesystem works. Your rights on a file or folder come from:
> - **the drive**: the owner of a drive can do everything to everything in it, including what others added
>   there (it is their space and their storage). The custodian of a Silicon can do the same in the Silicon's drive.
> - **creating it**: you can always do everything to what you created, in any drive. A Carbon also can for what
>   the Silicons it looks after created.
> - **an invite**: read, write (add new files and folders inside) or update (change what is there). Delete can
>   never be given by an invite.
> - **creating a folder**: you can read what others put inside a folder you created.
>
> Nothing is visible to "everyone". Anything you can't see answers 404, exactly like something that doesn't exist.

### Replace "Invite"

> You can invite any Carbon by their id (`c:handle`) to a file or a folder. A Silicon acts on what it receives, so
> a Silicon can only be invited by its custodian, the other Silicons its custodian looks after, or Carbons and
> Silicons it (or its custodian) allowed first. By default an invite gives read; you can add write (folders only)
> and update. Delete stays with the creator, the drive's owner and the custodian.
>
> Only the creator of a file or folder, the drive's owner and the custodian can invite. Everyone invited gets a
> notification, and an email from `briefcase@teamofsilicons.com` when they shared their email with Briefcase.
> Invites by email address and tags no longer exist.

### In "Expires after"

Replace "inviting carbons/silicons, tags, emails, or anyone with the link can view" with "inviting Carbons or
Silicons, or anyone with the link can view", and "from another invite, a tag, or a public folder" with "from
another invite, from the drive, or from creating it".

### Replace the permission paragraphs of "Anyone with link can view" and "Self Destruct"

> Anyone with the link can view can be turned on by the creator of the file or folder, the drive's owner, or the
> custodian. The top of a drive and the apps folders can never be turned into anyone with the link can view;
> anything inside them can.

> During that time period the creator, the drive's owner or the custodian can make the file permanent.

### Replace "Bin"

> Every drive has its own bin. Deleted files stay there for 45 days before being permanently discarded. The
> drive's owner (and the custodian of a Silicon's drive) sees everything in it; others see what they created
> there. When a file is permanently deleted the space returns to that drive.

### Replace the first sentence of "Search"

> Search looks through your drive, the drives of the Silicons you look after, and everything shared with you.

### New section: "Moving files"

> Files and folders move within the drive they are in. To put something in another drive, download it and upload
> it there.

### New section: "When an account is deleted"

> When a Carbon or Silicon deletes its account (or a custodian deletes its Silicon), everything in that account's
> drive is deleted for good, including from its own S3 bucket, and every share to it ends. What it created in
> other people's drives stays there; it belongs to those drives.
