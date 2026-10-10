#!/usr/bin/env python3
"""Local S3 compatibility fixture; requires moto[server]==5.2.1.

Moto omits checksum types on PutObject/CopyObject/HeadObject and serializes the
multipart completion with a Response root instead of Result, without checksums.
Emulate those AWS response fields here, without weakening the service's checksum
validation. This is emulator evidence, never proof of AWS compatibility.
Run: python scripts/local-s3.py (127.0.0.1:4102).
"""

import moto
from moto.s3.responses import S3Response
from moto.server import main

if moto.__version__ != "5.2.1":
    raise RuntimeError("This compatibility fixture is pinned to moto==5.2.1")

original_put = S3Response.put_object
original_copy = S3Response.copy_object
original_head = S3Response.head_object
original_post = S3Response._key_response_post


def put_with_checksum_type(self):
    status, headers, body = original_put(self)
    if status == 200:
        key = self.backend.get_object(self.bucket_name, self.parse_key_name())
        if key.checksum_value:
            key.checksum_type = "FULL_OBJECT"
            headers["x-amz-checksum-type"] = "FULL_OBJECT"
    return status, headers, body


def copy_with_checksum_type(self):
    status, headers, body = original_copy(self)
    if status == 200:
        key = self.backend.get_object(self.bucket_name, self.parse_key_name())
        if key.checksum_value:
            # Briefcase requests SHA256 explicitly, so Moto computes this over
            # the complete copied bytes even if the source was multipart.
            if self.headers.get("x-amz-checksum-algorithm"):
                key.checksum_type = "FULL_OBJECT"
            if key.checksum_type:
                field = f"<ChecksumType>{key.checksum_type}</ChecksumType>"
                body = body.replace("</CopyObjectResult>", field + "</CopyObjectResult>")
                headers["content-length"] = str(len(body.encode("utf-8")))
    return status, headers, body


def head_with_checksum_type(self, bucket_name, query, key_name, headers):
    status, response_headers, body = original_head(self, bucket_name, query, key_name, headers)
    if status == 200 and headers.get("x-amz-checksum-mode") == "ENABLED":
        key = self.backend.get_object(bucket_name, key_name, version_id=query.get("versionId", [None])[0])
        if key and key.checksum_value and key.checksum_type:
            response_headers["x-amz-checksum-type"] = key.checksum_type
            if key.checksum_type == "COMPOSITE":
                response_headers[f"x-amz-checksum-{key.checksum_algorithm.lower()}"] = f"{key.checksum_value}-{key.checksum_parts}"
    return status, response_headers, body


def post_with_multipart_result(self, request, body, bucket_name, query, key_name):
    status, headers, result = original_post(self, request, body, bucket_name, query, key_name)
    if status == 200 and query.get("uploadId"):
        result = result.replace("CompleteMultipartUploadResponse", "CompleteMultipartUploadResult")
        key = self.backend.get_object(bucket_name, key_name)
        if key and key.checksum_algorithm and key.checksum_value and key.checksum_type:
            name = f"Checksum{key.checksum_algorithm}"
            checksum = key.checksum_value
            if key.checksum_type == "COMPOSITE":
                checksum = f"{checksum}-{key.checksum_parts}"
            fields = f"<{name}>{checksum}</{name}><ChecksumType>{key.checksum_type}</ChecksumType>"
            result = result.replace("</CompleteMultipartUploadResult>", fields + "</CompleteMultipartUploadResult>")
        headers["content-length"] = str(len(result.encode("utf-8")))
    return status, headers, result


S3Response.put_object = put_with_checksum_type
S3Response.copy_object = copy_with_checksum_type
S3Response.head_object = head_with_checksum_type
S3Response._key_response_post = post_with_multipart_result
main(["-H", "127.0.0.1", "-p", "4102"])
