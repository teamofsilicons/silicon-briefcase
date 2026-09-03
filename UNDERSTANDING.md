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

Logging in and signing up are handled entirely by Silicon IAm (this is our access and authorization management layer). You would have an app_id and app_secret stored in your env that you can use to request the login and signup from Silicon IAm (read [https://backend.iam.teamofsilicons.com/docs/client/]) you would realise how you would need to login and singup using silicon IAm. For both signing in and signing up into the system would need Silicon IAm authorization, once you have the access token from SIlicon IAm for the user logged in, render the application accordingly. 

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
`

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



# Folder creation

Folders can be created by the ones who have full access to a said folder, and have the update permission there. Ones with only the read permissions won't be able to create folders.

For the folders at the base directory it would require you to define the folder type amongst the give folder types we have. 

For private folders, you can also explictly state the carbon_id's or silicon_id's for the members you wanna invite. Inviting should only be possible for the carbons and silicons that are already part of the org.


# Permissions

Similar to how linux_filesystem works, we would also have a very similar workflow. For each private or tag based file or folder it would be possible to invite someone, while inviting the invite could either be just read access or also update access or also write access. So the scope of the invited person would be defined there. Based on the scope it should appear accordingly in the user's directory. 

Each carbon/silicon that gets access to the permanent url and still don't have the access to the file, would see a button to request access clicking on request access would create a request for the owner of the file/org_admin/org_owner which they can approve and would give that carbon/silicon a view access or update access based on the request. 

I should be able to request permissions of file(s) or folder(s) so that it's clear what all actions can be performed on this. 

Just because someone has update permissions to a file doesn't mean they can delete it. They explicitly need the delete access to be able to delete it, same goes for update and write. Read just let's them view/download the file. 

# Notifications

There should be a centeral notification system where it would contain all the information regarding when any carbon or silicon recieves access to the any new file or folder or added to a new file, or change in permissions for something, all of it should be reflected here. There should be endpoints to make the entire notification inbox mark as read. It should return the 20 latest notifications when fetched along with how many new notifications in a number that will be used to display the badge. 


# Url

For each file there would be a permanent url, this is the url that would request the authenticated user's token to check if the person has access to the file and is rendered only if they have access to the file, otherwise it returns file not found. 

The said url is gonna be a clean url so it's gonna show the folder structure very clearly, for eg for a file shared from org tos from private folder of cos:tos with the folder name top_secret and file name this_secret.md. The url of the stored file would look like:
`briefcase.teamofsilicons.com/org/tos/private/cos:tos/top_secret/this_secret.md/`

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


# Version History

For all the updates in a file, a version history should be maintained for it for the last 50 versions. 

# Bin 

For any file deleted, they should be stored in the bin for 45 days before being permanently discarded. When a file is permnanently deleted the space should return to the the total availaible space. 

# Contents

I should be able to navigate and also request for every file inside that folder if i have access to it, i should be able to print all the contents. 

For each content return the latest 100 entries, it's paginated so should be possible to ask for the next batch in case of more results. 


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

has: is content-only, matched against extracted document text.

contains: is name or content. The contract mentioned contains: separately with the glob note but never said what it searches, so it became the union.

name: is name-only — not in the contract at all. It exists to complete the trio: name-only / content-only / either.

location: as an anchored path prefix with `*`;

the permissions:/permission: value set (read, write, update, delete, manage_permissions/manage);

the boolean grammar (implicit AND, or, not, leading -, parentheses); last:/first:/sort: being top-level only; and the limits — take ≤ 100, expression ≤ 1,024 bytes, ≤ 32 predicates.

# How other apps would use Briefcase

Refer to the OBO access on [https://backend.iam.teamofsilicons.com/docs/client/], we will need to expose a list of requests that the other applications should be able to perform on us. We will expose the endpoints to:
1) create a new file at any location for that user.

### How to store app specific data

For everything created by a specific application inside the authenticated silicon/carbon folder  create an apps folder where based on the app_id you create a folder and store everything there by default.


`For every file and folders created, accessed, deleted, updated or downloade maintain a version history that would store who performed the said action and the timestamp, maintain the history upto the last 100 entries.`

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

if you need a local store for auth or something else, use ~/.{appname}/ dir.

For both package and the cli write detailed docs on how to use the package and how to use the cli, and also another doc on how to use the package. 

Package and CLI must only expose the client side actions, and not the internal actions performed by the backend. For the CLI follow the standard command line grammar rules, and also include a -h command that shows all the possible commands.