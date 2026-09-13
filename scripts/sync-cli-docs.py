#!/usr/bin/env python3
"""Keep the published CLI's offline manuals identical to the source guides."""
from pathlib import Path
import sys
root = Path(__file__).resolve().parent.parent
pairs = {'cli': 'cli/README.md', 'client': 'client/README.md', 'testing': 'testing-environments.md'}
check = '--check' in sys.argv
stale = []
for name, source in pairs.items():
    src = root / 'docs' / source
    dest = root / 'clients/rust/crates/briefcase-cli/manual' / f'{name}.md'
    if check:
        if not dest.exists() or dest.read_bytes() != src.read_bytes():
            stale.append(name)
    else:
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(src.read_bytes())
if stale:
    sys.exit('Bundled manuals are stale: ' + ', '.join(stale) + '. Run python3 scripts/sync-cli-docs.py')
