"""Read-only save reminders. Called on Fusion's main thread after command completion.

Exports are checkpoints, not proof that Fusion cleared its modified/recovery state.
This module never saves, closes, infers ownership from names, or deletes recovery data.
"""


def _property(obj, name):
    try:
        return getattr(obj, name)
    except Exception:
        return None


def inspect_documents(app):
    documents = []
    try:
        for i in range(app.documents.count):
            doc = app.documents.item(i)
            saved = _property(doc, 'isSaved')
            modified = _property(doc, 'isModified')
            documents.append({
                'name': _property(doc, 'name'),
                'isSaved': saved, 'isModified': modified,
                'needsSave': modified is True or (saved is False and modified is not False),
                'saveStateKnown': saved is not None and modified is not None,
            })
    except Exception as exc:
        return {'inspectionComplete': False, 'documents': documents, 'error': str(exc)}
    pending = [d for d in documents if d['needsSave']]
    return {'inspectionComplete': True, 'openDocumentCount': len(documents),
            'needsSaveCount': len(pending), 'documents': documents,
            'recoveryHealth': 'notInspected', 'ownership': 'notInferred'}


def attach_document_hygiene(app, result):
    if not isinstance(result, dict):
        return result
    state = inspect_documents(app)
    result['documentHygiene'] = state
    count = state.get('needsSaveCount', 0)
    if count or state.get('openDocumentCount', 0) > 3:
        names = [d['name'] for d in state['documents'] if d['needsSave']]
        hint = (
            'Document checkpoint: %s open, %s need saving%s. '
            'Save edited work at milestones and before ending the task. A source export '
            'or Document.CopyToDesktop is a separate checkpoint and may leave Fusion '
            'isSaved=false/isModified=true; verify the artifact and re-read save state. '
            'Close only this task\'s completed scratch documents by exact identity with '
            'fusion_close_document after verifying a current native checkpoint; never '
            'bulk-discard other work. If Fusion reports recovery-file failure, checkpoint '
            'immediately, then save or close that document; do not just dismiss recovery. '
            'Use a task subfolder when saving to cloud.'
        ) % (state.get('openDocumentCount', '?'), count,
             (' (' + ', '.join(str(n) for n in names[:6]) + (', ...' if len(names)>6 else '') + ')') if names else '')
        result['_hintDocumentHygiene'] = hint
        result['_hint'] = (result.get('_hint', '') + ' ' + hint).strip()
    elif not state['inspectionComplete']:
        result['_hintDocumentHygiene'] = 'Save-state inspection incomplete; inspect documents before cleanup.'
    return result
