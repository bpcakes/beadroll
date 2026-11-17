"""What the drivers compare of a Beads record: the record as two exports of one tracker must agree on it."""
import json


def canonical(record):
    """`record` without `source_repo_path`, which each export writes for its own checkout, and with its labels
    and dependencies in one order: sorted by their JSON, a total order, so that two lists holding the same
    members compare equal whatever order each was written in."""
    record = dict(record)
    record.pop("source_repo_path", None)
    for field in ("labels", "dependencies"):
        if field in record:
            record[field] = sorted(record[field], key=lambda value: json.dumps(value, sort_keys=True))
    return record
