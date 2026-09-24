# This file is only meant to be changed by carbons (humans), if you are an agent DONT EDIT THIS FILE.  


# UNDERSTANIDNG.md - briefcase

This is the understanding of briefcase - our file management system. This is the platform that all the other apps, and the entire organisation would use to manage all their files. 

Both Silicons and carbons would be using this system. 


# Glossary

`Carbon` - The human in the system. Every human account is called a carbon.
`Silicon` - Our AI Agent (silicon) account is refered to as a Silicon.
`Org` - This is our organisation, this is where all the silicons and carbons would stay for a single organisation and defines the scope. 


# How it works

For each organisation there would be an organisation directory under which we are gonna manage all the files and manage their systems. For each file there would be CRUD operations and who can perform each of these C, R, U, D operations. 

The client just needs to upload internall we decide:

Files up to and including 100 MiB should be uploaded in a single request.

Files larger than 100 MiB should use S3 multipart upload.

Algorithm to follow to define multipart upload:

target_part_count = 1,000
minimum_part_size = 8 MiB
maximum_part_size = 5 GiB

calculated_part_size = ceil(file_size / target_part_count)
part_size = clamp(round_up_to_nearest_MiB(calculated_part_size), 8 MiB, 5 GiB)
number_of_parts = ceil(file_size / part_size)


Example:

| File size |     Part size | Parts |
| --------: | ------------: | ----: |
|    50 MiB | Single upload |     1 |
|   200 MiB |         8 MiB |    25 |
|     1 GiB |         8 MiB |   128 |
|    10 GiB |        11 MiB |   931 |
|   100 GiB |       103 MiB |   995 |

Each organisation has an upload limit of 100gb per day (resets at 12:00am utc), and an organisation wide maximum total storage of 1 peta byte. The total storage limit and upload limit should be configurable per organisation by a variable in the database itself, so by default it's gonna be 100gb and 1 petabyte but can be configured per organisation. 

For every upload it should be possible to define the exact path and folder where i wanna store this file, this path would be defined from the base. 

It shouldn't be possible to save files in:
1) the base directory where private and public exist.
2) directly inside the private directory
3) inside folders assigned to other carbons/silicons/tags. Basically places where i dont have the access to. 

There should also be endpoints that shows how much are they currently using not in percent but actual space they are currently consuming. 

# How Does login/signup work

Logging in and signing up are handled entirely by Silicon IAm (this is our access and authorization management layer). You would have an app_id and app_secret stored in your env that you can use to request the login and signup from Silicon IAm (read [(https://github.com/teamofsilicons/silicon-iam/tree/main/docs/client)]) you would realise how you would need to login and singup using silicon IAm. For both signing in and signing up into the system would need Silicon IAm authorization, once you have the access token from SIlicon IAm for the user logged in, render the application accordingly. 

Use the oficial and latest silicon client for using IAm at all times and across everywhere. (https://crates.io/crates/silicon-iam-client/)

The webhook endpoint ([backend.briefcase.teamofsilicons.com/webhook/]) you have would give you information whenever someone logs out, kicked from org, anything changes you would know.

# How organisations are defined

Organisations are defined entirely inside Silicon IAm. Both organisation and all the users that have access to the organisation, their tags, their trust. 

For the members that previously had access and now are kicked their access should instantly be revoked, you would know via the webhook endpoint defined. 

Once the user has signed in they would also be able to create organisation that would again directly take them to IAm where they can configure the organisation, invite, etc. 


# How to store data

For storing all the data we have an s3 bucket where it will all be stored. For each organisation configuration it would also be possible to be able to define if they would like to use their own S3 bucket instead. This would be an entire configuration step, the steps to follow:

`info needed`:  Bucket name, AWS region, IAM role ARN, Bucket prefix, AWS account ID, Encryption mode

Once we get all the information, we do a demo upload and do all CRUD operations on them at the end we leave the file deleted. This is a test to check if the s3 bucket is working as intented and not failed. 

Once this is confirmed we will show S3 bucket configured. And will use that S3 bucket for that organisation. 

Frontend Note: Don't display this option by default when they specifically go inside organisation config there they should see the option to configure your s3 bucket. 

---

Otherwise use our own s3 bucket as configured in the env. This s3 would have configured in a folder way for each organisation having one folder, it shouldn't be possible to be able to access another organisation. This organisation's folder is the base at which the system loads. 


# Files supported

Every, any file type can be uploaded.

Frontend should support render for: 
any image type (png, jpg, jpeg, webp, gif, svg, avif, heic, tiff, bmp, ico, etc.)  
any video type (mp4, mov, webm, mkv, avi, mpeg, mpg, m4v, 3gp, ogv, etc.)
any document type (pdf, doc, docx, odt, rtf, txt, md, pages, tex, etc.)  
any spreadsheet type (xls, xlsx, csv, tsv, ods, numbers, etc.)  
any presentation type (ppt, pptx, odp, key, etc.)  
any audio type (mp3, wav, m4a, aac, flac, ogg, opus, wma, aiff, amr, etc.)  
any archive type (zip, rar, 7z, tar, gz, gzip, bz2, xz, tgz, iso, etc.)  
any code or data type (json, xml, yaml, yml, html, css, js, ts, py, java, sql, log, etc.) 

Ensure to render them all in a sandboxed enviorment. 

For images open a detailed view, for videos they should have a good player, for documents it should be correctly rendered in place, for spreadsheets they should be visible in the correct format and well organised, for presentations also render them, for audio create an audio player, for any archives, dont unarchive it but let them be able to view the contents, for code or data type render in the correct syntax. 

For any other file types that are not recognised just dont render but CRUD operations can still be done on them, just display Unsupported file type for them all. 


# Special folder types

`Public`:  For every public folder, it means that every single file inside this folder can be ready by anyone in that organisation. This just gives every carbon and silicon in the organisation the read access to that file. `Create, Update and Delete` are still restricted.  

`Private`: For Private folder, it means that all the files inside this folder can't be viewed without the explicit permission to view. 

`Tag`: For the tags in the organisation, each tag would have it's own folder and everyone with that tag should be able to view the files inside that folder, or create files inside that folder.  

These are folder types and names have nothing to do with them


For every folder/file inside Public it would be public, for every folder/file inside Private it would be Private, for every folder/file inside Tag it would Tag. 

# Folder Structure

As soon as anyone enters in the system they would see a Public folder, a Private folder, and a folder for all the tags. Folders created on this level would need to know what kind of folder is being created. 

### Inside Public

All the files and folders inside public are public and can be viewed by all the the org members. Anyone who opens this should see all the files and folders inside the public folder. Anyone would be able to upload to this folder. 

### Inside Private

Inside the private folder for all the files and folders there would be permissions assigned accordingly, who can read, who can update. 

Inside private there would be a folder for all the carbons and silicons. One for each carbon and silicon id. Only display to users a folder of another carbon/silicon only if they have a file/folder shared with the logged in user. Otherwise keep such folders hidden. 

For if a file is shared inside a folder, then fetching that folder directly as an user should return the files they have access to. If their access to all the files inside a directory is lost it should then start returning 404 on fetching the folder. 

`Frontend note: For all the private folders except the user's own carbon_id/silicon_id add a small i box at the bottom most - You might not be seeing all the contents of this folder. This is a permission based folder.

### Inside Tags

For each tag based folder, by default at the same level of public and private there would be a folder for each tag, a user should only see the folder of the tags that belong to their tag. 

`Frontend note: If a specific user is invited to a file/folder that doesen't belong to their tag, display the same info note as we show in private folder.`

### Inside Apps

Apps maintain a first level copy of the main layer. For each application it would maintain it's public which can be viewed by anyone, private which would have specific carbon and silicons folder, which would follow the same concept to how normal private folder works.


For eg: For an Org TOS with 3 carbons and 2 silicons and 2 tags this would be the structure. With carbon A, B and silicon A with the access of Tag 1, and Carbon  C and Silicon B with Tag 2 

Public (accessible by all 3 carbons and 2 silicons):
	X Folder
	Y Folder
	secret.mp4
	company_docs.md

Private:
	Carbon A (visible to carbon a, c, and silicon a):
		cats.mp4 (shared with Silicon A)
		secret.md
		no_one_knows.mp3 (shared with Carbon C)
	Carbon B
	Carbon C
	Silicon A
	Silicon B

Tag 1 (accessible by carbon a, b, and silicon a):
	special_tag_1.md
	nums.csv

Tag 2: (accessibly by carbon c, and silicon b):
	tag_2.mp4

Apps:
	`<app-id>`:
		public:
			X Folder
			Y.txt
		private:
			Carbon A (same concept as normal A)
			Carbon B
			Carbon C
	`<app-id-2>`:



# Folder creation

Folders can be created by the ones who have full access to a said folder, and have the update permission there. Ones with only the read permissions won't be able to create folders.

For the folders at the base directory it would require you to define the folder type amongst the give folder types we have. 

For private folders, you can also explictly state the carbon_id's or silicon_id's for the members you wanna invite. Inviting should only be possible for the carbons and silicons that are already part of the org.

# Update

When in the CRUD someone is performing U(updating) a file it means that they are adding in a new version for the file, this should update the file in it's own place with the new version of the file, and from that point on the new version of the file should be displayed. 

# Permissions

Similar to how linux_filesystem works, we would also have a very similar workflow. For each private or tag based file or folder it would be possible to invite someone, while inviting the invite could either be just read access or also update access or also write access. So the scope of the invited person would be defined there. Based on the scope it should appear accordingly in the user's directory. 

A member without access to a permanent URL sees File not found. An owner or authorized administrator can grant access through sharing.

I should be able to request permissions of file(s) or folder(s) so that it's clear what all actions can be performed on this. 

Just because someone has update permissions to a file doesn't mean they can delete it. They explicitly need the delete access to be able to delete it, same goes for update and write. Read just let's them view/download the file. 

# Notifications

There should be a centeral notification system where it would contain all the information regarding when any carbon or silicon recieves access to the any new file or folder or added to a new file, or change in permissions for something, all of it should be reflected here. There should be endpoints to make the entire notification inbox mark as read. It should return the 20 latest notifications when fetched along with how many new notifications in a number that will be used to display the badge. 


# Url

For each file there would be a permanent url, this is the url that would request the authenticated user's token to check if the person has access to the file and is rendered only if they have access to the file, otherwise it returns file not found. 

The said url is gonna be a clean url so it's gonna show the folder structure very clearly, for eg for a file shared from org tos from private folder of si:cos with the folder name top_secret and file name this_secret.md. The url of the stored file would look like:
`briefcase.teamofsilicons.com/org/tos/private/si:cos/top_secret/this_secret.md/`

Whenever someone requests from this url it should only be rendered if the user has the permissions to view the file. Or see the options accordingly for when they can perform other CRUD operations.

For all permanent url it should always have the org in it, so the base url would be per organisation: `briefcase.teamofsilicons.com/org/{org_id}/` and configured further accordingly. 

The backend is served on backend.briefcase.teamofsilicons.com but the permanent url is servered from briefcase.teamofsilicons.com. 

# Download

Anyone with read access to the file should be able to download the file locally. 


# No access

For the files and folders the user doesen't have access to they should return 404 to the user like the file doesen't exist, it should never say you don't have access to it, it should just return like that file/folder doesen't exist.

# Search

There would also be user specific search, for these searches it should be possible for the user to be able to do a file search, this search should also consider the contents of the documents and also suggest such files if the document content matches, for each document that the content matches also return how many hits in the document. For any given search it should return 0-20 results. For the priority the highest would be if the file name matches, second documents with the highest number of matches gradually falling off. 


# Org_admins and Org_Owners

For org_admins and org_owners they would have gods eye view, any file or folder, private, or non private, org_admins and owners should be able to do all the CRUD operations on them. They should also be able to see ALL the files and folders. 


# Version Management

For all the updates in a file we maintain a version history for it all. Everytime there's a new version for each file when it's updated is when a new version is added the version must be managed. For managing versions we use file hashing. Also store the current version number that would be incremental 1, 2, 3, 4, and so on. 

# Folder retrieval 

It should be possible to retrieve or download entire folders that can be done and that would download the folder in the tar.zst format. This should be doable even via the api, the cli or the client where we will send them the tar.zst format over. 

# Bin 

For any file deleted, they should be stored in the bin for 45 days before being permanently discarded. When a file is permnanently deleted the space should return to the the total availaible space. 

# Contents

I should be able to navigate and also request for every file inside that folder if i have access to it, i should be able to print all the contents. 

For each content return the latest 100 entries, it's paginated so should be possible to ask for the next batch in case of more results. 

# Invite

I should be able to invite other silicons or carbons or specific email to view my file/folder, i can invite them to a folder or i can invite them to a specific file. During invitation i can invite any carbon/silicon of my org using their id or invite tags so anyone with the tag should be able to view the file, during inviting i can also set the permissions by default they just have the Read permission but i should be able to individually configure if i wanna give them any other permission like Create or Update. Create would only be valid for folder invites. Delete is still reserved to the creator and org admin and org owner. 

For each invited carbon or silicon also send them a mail that you have been invited to this file/folder. We use postmark to send the emails. You would send the email via `briefcase@teamofsilicons.com`. For the emails you send ensure to send it to the email they are registered in the org with and not the main personal email until both match. 

# Expires after

For each file or folder during sharing i can set it to expire after a set time (an expiring share), this wont be enabled by default but can be set as an expiring share, in which i can set a time between 1 min and 1 month (30 days), in steps of 1 minute. Any kind of share can be an expiring share: inviting carbons/silicons, tags, emails, or anyone with the link can view. An expiring share only ever gives Read access (view/download).

And i could list down the people or the type of share that i wanna do and that share would be valid for the set time and after that time the access for those set of people (and only that set of people invited in that run) will be revoked. An expiring share is its own grant: if someone also has access from another invite, a tag, or a public folder, that access stays after the expiring share expires.

Expiry is strict: the moment the time passes the share must stop working, checked on every request, not left to a cleanup that runs later.

Before it expires it should be possible to extend it, shorten it, make it permanent, or revoke it early.

No email or notification is sent when an expiring share expires. Creating, changing and expiring an expiring share are all maintained in the logs. Apps can also create expiring shares through the OBO endpoints (invite, and anyone with the link can view).

# Self Destruct

While uploading a file i can set a file to be self destruct (1 min to 1 month (30 days), in steps of 1 minute) and then after that much time the file will get auto deleted for good (permanently). It doesn't go to the bin, and its space returns to the total available space immediately. The timer starts when the upload finishes.

Self destruct can only be set while uploading, it's not possible to turn it on for an already existing file. Uploading a new version of the file doesn't change the timer.

If the file is deleted by hand before the timer runs out, it is also permanently deleted and doesn't go to the bin.

During that time period there should be an option to be able to make the file permanent. Only the creator, org_admins and org_owners can make it permanent.

Setting self destruct, making the file permanent and the deletion are all maintained in the file and folder logs. No warning is sent before the file gets deleted.


# Filter

For filtering the following options should be possible:

```
last:N / first:N                  take N, chronologically
between:DD-MM-YYYY=DD-MM-YYYY     both ends inclusive
after:DD-MM-YYYY / before:DD-MM-YYYY
from:@{...} / to:@{...} / for:@{...}
contains:'...'                    `*` is a glob: contains:'confirm*'
sort:newest / sort:oldest         oldest last by default
is:X / has:X                      is could define file types, has can define                                       content
permissions
location
```

created-by / shared-with / accessible-to = from:@{...} / to:@{...} / for:@{...}

There can be any possible PnC for the filters, i should be able to combine multiple filters. Filters should be super powerful, it should be possible to filter anything out, i should be able to filter niche things, for eg: filter out the most recent 5 files in the last 10 days or between 12 june 2026 and 12 july 2026 from the '/private/' folder that contain the word "apple" or "cat" and it must be in an .md file. 

FIltering should only happen with the files i have access to.

is: takes three vocabularies at once. Entry kind — is:file, is:folder (is:directory aliased). Renderer category — is:image, video, document, spreadsheet, presentation, audio, archive, code, unsupported, i.e. the nine buckets from §Files supported. Anything else alphanumeric and ≤16 chars falls through to a file extension, leading dot stripped (src/domain/filter.rs:727). So is:document is any file that opens in the document renderer — pdf, docx, md; is:md is literally .md.

is: also takes two lifetime values: is:expiring matches files and folders with an active expiring share, either one that gives me my access or one i can manage; is:self-destruct matches files whose self destruct timer is still running. These are checked before the extension fall-through, so is:expiring never means an .expiring extension.

has: is content-only, matched against extracted document text.

contains: is name or content. The contract mentioned contains: separately with the glob note but never said what it searches, so it became the union.

name: is name-only — not in the contract at all. It exists to complete the trio: name-only / content-only / either.

location: as an anchored path prefix with `*`;

the permissions:/permission: value set (read, write, update, delete, manage_permissions/manage);

the boolean grammar (implicit AND, or, not, leading -, parentheses); last:/first:/sort: being top-level only; and the limits — take ≤ 100, expression ≤ 1,024 bytes, ≤ 32 predicates.

# Anyone with link can view

For each file or folder i should be able to set that anyone with the link can view which would make it truly public and the user doesent need to be of the organisation to be able to view this file. Anyone with the link can view just gives the view and download permissions to the file/folder. 

For configuring a folder/file for anyone with the link can view i should have the org_admin, org_owner or the creator of the file permissions. 

Just having the link would give them permission to view the file if any file or folder has anyone with the link can view configured. 

For a folder if it's turned into anyone with the link can view, all the files and folders inside it automatically get turned into anyone with the link can view. 

It shouldn't be possible to turn someone's entire private folder to anyone with the link can view, so org/private or org/private/<carbon/silicon-id> can't be turned into anyone with link can view under that it's possible to turn all file and folders that the user has correct access to into anyone with link can view. 

# Logs

For each file or folder maintain logs, in logs maintain every carbon/silicon who was invited, if someone's permission was updated, if any files were deleted inside folder maintain the logs for it as well, any file added should also be stored in the folder logs. And in the file logs maintain specific update logs, version rollback logs, permissions, inviitation, anyone with the link can view, etc. Maintain logs for past 365 days. 

# Versioning

For versioning we have Contract Governance/API/service contract lifecycle management. We will have:

1) Contract versioning / API versioning
2) Protocol Negotiation
3) Backward compatibility
4) Consumer-driven contract testing
5) Deprecation and sunset management - if 0 requests for 7 days, sunset that version
6) Compatibility matrix
7) Version policy


# How other apps would use Briefcase

For other apps to use Briefcase, we would configure our OBO endpoints in Honeycomb. Briefcase would still verify the proof with IAM and check what the user can access before performing the action. We will expose the endpoints to:
1) Create a new file for that user in the private folder - non critical endpoint
2) Create a new file in public folder for that user - non critical endpoint
3) Read/Update/Delete the file of that user in the specific app directory - non critical endpoint. This only let's the app perform actions inside the specific app directory only for the files that carbon/silicon has the appropriate access to. 
4) Invite other carbons/silicons to a file/folder of that user - Critical endpoint
5) Make the file/folder anyone with the link can view - Critical endpoint

For all of these actions they can only be performed as the user in the specific app directory that app has. 

### How to store app specific data

For all the app specific data and even the scope of the said application is just for that specific application folder inside the app/ directory, and even inside that they should only be able to perform actions the user should have been able to perform. 


`For every file and folders created, accessed, deleted, updated or downloade maintain a version history that would store who performed the said action and the timestamp, maintain the history upto the last 100 entries.`


# Testing Environment

For testing we would use the environments managed by Honeycomb. IAM would still handle the test identities, login and authorization, and Briefcase would handle its own test files and permissions.

A test environment is basically the same Briefcase where I can test uploading files, deleting stuff, sharing folders, and checking the user's permissions. It starts empty and uses the same APIs and workflows with completely isolated data.

### Environment Lifecycle

Briefcase would accept authenticated instructions from Honeycomb to prepare the environment, update its key version, clean, disable, restore and permanently remove its test data. Keep the same environment_id across the services. Each operation must be safe to retry and report whether Briefcase's work is pending, completed or failed. These instructions must work even when the test sessions have been disabled.

For creation and restoration, Briefcase would only allow test access once Honeycomb confirms all required services are ready. If IAM enforces this shared readiness, Briefcase must check that current IAM state before allowing access. Finishing its own preparation alone does not make the environment ready.

Cleaning keeps the environment but clears its files, versions, permissions, uploads and other test data. Block access while cleaning and check the environment revision and cleaning generation so old requests, jobs or webhooks cannot bring back cleared data. Only report completion once the required storage cleanup has finished. Cleaning must keep Briefcase linked to the environment, so later deletion, restoration and permanent removal still reach it even before apps are reimported.

Honeycomb decides inactivity expiry and the recovery period. Briefcase reports activity and carries out the cleanup instead of independently retiring the shared environment. Disabling blocks access immediately, restoring makes retained data available again when authorized, and permanent removal clears the remaining storage. Restoring cannot undo a clean.

### Using a Test Environment

In the client app, website, CLI, or API, passing the test environment’s `app_secret` would select that application’s test environment. No manual pairing or separately entering the environment root key should be needed. Briefcase should validate the secret with IAM and identify the correct environment automatically.

For logging in, it would ask for an SLT. In a test environment, this can either be an IAM-issued test SLT or the public ID of an existing Carbon/Silicon in the test sandbox. Entering the ID would sign me in as that test user. Unknown or inactive identities should be rejected. This shortcut must never work in production.

The testing_key managed by Honeycomb gives administrative control over the test world. The application’s `app_secret` selects its sandbox. Once signed in as a particular user, actions must follow that user’s actual permissions. Possessing the secret must not make every signed-in user bypass permission checks.

If an administrative or god view is provided, it should be separate and clearly labelled so it cannot be confused with testing what a normal user is allowed to do.

### Website and CLI

Environment creation and management from the website, CLI or client would go through Honeycomb. Briefcase would still let me enter an existing environment and use its normal file commands.

On the website, I should be able to enter the `app_secret` from settings or the sign-in screen. Without a selected test environment, the application would use production.

When in a test environment, always show a banner at the top saying that I am currently in a test environment, along with its name, the signed-in test identity, and a button to exit testing mode.

Production and testing sessions should remain separate. Exiting testing mode should return me to the production session or ask me to sign in.

In the CLI, always display the selected test environment at the end, including when a command fails. This message should go to stderr so it does not interfere with JSON output, downloaded files, or commands used in scripts.

### Isolation

Everything belonging to a test environment must stay inside that environment, including files, permissions, versions, deleted items, search results, caches, notifications, background jobs, and audit logs.

Production credentials must not work in testing, and credentials from one test environment must not work in another.

If a supplied test secret is invalid, revoked, or belongs to an unavailable environment, return an error. Never silently continue in production.

### Storage and Environment Limits

Each Briefcase test environment would have a maximum total storage of **2 GiB**. This includes retained file versions, deleted files that have not been permanently removed, and storage reserved for uploads.

Concurrent uploads must also respect this limit. If an upload would exceed it, return:

`In test enviorment you are limited to a total storage of 2gb per enviorment.`

There would be a maximum of **10 simultaneous active test environments across the Silicon Briefcase deployment**. Retired environments would not count as active, and restoring one would require an available slot.


### Webhooks and External Actions

Test webhooks should follow IAM’s documented format. Verify the signature over the complete raw body, identify the correct test environment, and apply the event only there. Duplicate or out-of-order events must not corrupt the current state.

Test actions should not send real emails, SMS messages, payments, or other production effects. These should use test destinations or simulated delivery.

Secrets must not appear in URLs, logs, audit records, or stored webhook payloads.

### Sharing Links

Sharing should behave like production, including returning the file or folder link when “Anyone with the link can view” is enabled.

A test link must resolve only within its intended test environment.

---
---
---
---
---
---
---
---
---
---
---
---
---

Only above this line is what the IAm backend would hold, below this would be the users of the backend, the client, the frontend, the cli, etc. 

# Rust Package & CLI

The Rust package & cli using that rust package are first hand client with an always running deamon if needed in the background. the UI will be a subset of the cli. make sure everything works via the CLI first, and then we'll make the UI. Everyone should be able to use the CLI/Rust Package (carbons, silicons, org, access keys, api keys, read, write, patch, delete, everything).

The rust package would be stateless whereas the cli would be statefull. CLI built on top of the rust package.

For how this CLI is built, rust as the programming language, but can use anything under the hood that is needed. Maybe rust, or node, or shell, as and when the work comes. That is decided by the implementor based on the work. If something requirs a UI (like graph, live, video, images etc). for that the UI has an endpoint that can be viewed/used/downloaded and the cli gives the link to that.

The primary Interface is the Rust Package. CLI is built using the Rust Package only and doesn't have any feature that the Rust package does not.

if you need a local store for auth or something else, use `{home_dir}/.{appname}/dir`.

The default home dir is `~`. If `SILICON_HOME` is present in the enviorment variables, use that as the home directory by default. 

For both package and the cli write detailed docs on how to use the package and how to use the cli, and also another doc on how to use the package. 

Package and CLI must only expose the client side actions, and not the internal actions performed by the backend. For the CLI follow the standard command line grammar rules, and also include a -h command that shows all the possible commands.

Testing in the test enviorment should also be possible via both cli, and the package. 

Testing enviorment in cli, for testing enviorment in cli i should just be able to `briefcase --test <test_id> <command>` infront of the same command and it should treat that as a test command. Same for test only commands even they would have the same style just without specifying --test for them would return this action is only possible for test enviorment.  

--- logging in via cli ---

For logging in via the cli or the package for any carbon/silicon you don't ask for their credentials or redirect them anywhere, instead you just request for their short lived token. This short lived token would then be used for the same login logic, the short lived token would be compared and you will get the refresh and auth token. 

For CLI login there should be this exact command: `briefcase login <slt>`.
And there should be an command to configure the home directory where the information is stored: `{home_dir}/.{appname}/dir`. This can be confitgure via `briefcase config home {location}`. If it's not a directory give an error not a directory.


It should also expose these specific commands:
1) `--help` which would give all the help documentation on how to use waveform. So the user should be able to run `briefcase --help` and get the help docs.
2) `iam --json` the user should be able to run  `briefcase iam --json` which returns `app_id` alongside other information.
3) `login status --json` the user should be able to run `briefcase login status --json`, reports successful authentication reports `authenticated: true`, alongside which carbon or silicon is it authenticated as.


# Cli experience

CLI is the primary way to interact with IAM Apps. It should be built for both Carbons & Silicons. Any other interface (like website) will be a subset of the CLI.

The cli should never ask for credentials from either silicon or carbon. it should just ask for short lived tokens that the user can generate from the official iam cli, or from the web where the the user is sent to auth concent screen.

CLIs get SILICON_HOME env variable where it should store all the details. Its home, so you should use that as base, and make their own hidden folders to keep their information.

Specific apps that could benefit from using ISI env variable should do that. eg: dm.

ISI are internal silicons. If silicon is a brain, then isi are parts of the brain. store this inside metadata, or main data if its super useful. ISI may or may not be present. make sure to not rely on it in such a way that things break. consider ISI as useful additional information.

every app cli must support the following commands:

`app iam --json` gives {app_id: "...", ...}

`app login "..."` takes in a short lived auth token generated by silicon interpretter.

`app login status --json` tells if its {authenticated: true, ...}


App Internals:
All apps are suggested to make a rust library which is stateless. then 2 things that uses the rust library: always running daemon, and a cli interface that talks to the daemon.

On the docs page, show `honeycomb install 'briefcase'` to install the CLI, followed by how to log in.

CLI design should be focused on giving details and helping finding the right command to use. CLI will often have lots of commands and it should be like a tree that can be traversed using --help.

CLI documentation should be bundled inside the cli itself. On each print of the cli documentation using --help or otherwise, it should show what this command is for, how its often used (perhaps in conjunction with other commands if applicable) and then a list of flags etc it takes in.

Follow the CLI grammar. These CLIs can be used by humans, but more often than not, it'll be used by an agent who prefers to know why something broke and so it can figure out ways to fix it. Don't just say something went wrong... tell it exactly what & why.

A good rule of thumb is: these CLIs are being made for someone who understands ins-and-outs of technology. Make like a programming language that gives very specific and helpful errors and outputs compared to a web interface where all errors are hidden until absolutely critical.

All CLIs must have a report bug feature that also optionally takes in a PR ref if the agent did not just find a bug but also patched it. 

briefcase report `<report-message>` --pr `<pr-link>` and if someone just reports the bug, without the pr, show them a message, you can also put a pr in the repo (`repo-link`). 

Everytime a bug is reported use postmark to mail [saketdev12@gmail.com, shubhastro2@gmails.com, bugs@teamofsilicons.com]

Since all TOS applications are open sourced, any bug can be discovered, replicated, patched and a pr can be raised. Allow all such edge cases be figured out by the agent instead of fixing it ourselves based on a bug report.

Only a bug report submitting is possible, but its encouraged to give a lot more details and also attach a PR if possible.

Give the information of the github repo, online docs, rust package, etc inside the cli itself.

The CLI as i told before is a tree of documentation. Show possible paths, and then let someone go deeper along with documentation.


# Docs

There are two kinds of documentations: informative & instructive.

Always keep instructive documentation up front, easy to use, direct with clear instructions & link to informative documents to know why its done this way. Instructive documents should be the landing point of the product for both carbons & silicons.

It can give carbon the instructions on how to install & use it, or how to ask their silicon to use it.

For silicons, it can be that, but also how to do a lot more with it. Esp. things like building on top of it. Make it very clear what is expected, what is mandatory and how does the system work.

Then the silicon can dig deeper into the informative documentation to know all the possible ways to do it, & why its done the way its done.

While both carbons and silicons can read the documentation, it'll likely be more silicon. So design it for silicons. The more reasons you give, the better a silicon would be at making a judgement call of how to do something.

Since all IAM apps can both be used as is, and also built on top of... its imp to write documentation for both. Usage docs & Development docs.

# Telemetry

All IAM apps use Space Station [https://spacestation.teamofsilicons.com/docs] for telemetry. Telemetry is opted-in by default but can be opted out from settings if the user wants.

Space Station is also a rust package which can be used from within the backend, or daemon, or cli to send telemetry.

Record as many things as you think might be useful to diagnose or follow traces later.

Since space station is just an event store, make sure to include all the source, step, progress, etc information inside each event. some of the system information is automatically added to the metadata so you need not add that.

push context-rich, self-contained events.

Space Station also support web, for web it has 2 possible pathways: analytics & events. Most of the Analytics is self captured and you can define a seperate event store from the web. Its possible that both web analytics and web events go to separate tables.


# Configurability

We ship highly configurable apps with sensible defaults. Very much like VS Code. flags to toggle / customize behaviors.


# Updates

For each Briefcase app release, provide one Honeycomb-compatible .tar.gz with [honeycomb.yaml](honeycomb.yaml) at the archive root and the prebuilt CLI for all required targets. Use this file to map the `briefcase` command for app_id `briefcase` to each target's executable. Before packaging, set the version to the app release version and include the binaries at the paths defined in the file. Run `honeycomb validate` and then `honeycomb pack` to create the archive. Refer to [Honeycomb docs](https://docs.honeycomb.teamofsilicons.com/) for packaging, installation and updates. The CLI uses the app's release version. Installation and updates would be handled by Honeycomb, so Briefcase must not run its own updater for a Honeycomb-managed installation. The Rust client package remains a normal project dependency.

# Identifier schema

Silicon IDs use `si:{silicon_id}` (for example `si:cos`), Carbon IDs use `c:{carbon_id}` (for example `c:saket`), and application IDs use the bare `{app_id}` (for example `briefcase`). The components after `si:` and `c:` are handles; each prefix appears exactly once. Silicon IDs and application IDs do not contain an organisation component. Organisation membership and application ownership are stored separately under `org_id`.

Outside the schema patterns above, fields and standalone placeholders named `silicon_id`, `sid`, `carbon_id`, or `cid` carry the complete prefixed public ID; `app_id` carries the bare application ID. This applies to authentication, API and CLI inputs and outputs, configuration, permissions, URLs, events and stored identity references. Where a CLI selector uses `@`, it precedes the complete ID, such as `@si:cos` or `@c:saket`.
