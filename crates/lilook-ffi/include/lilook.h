/* lilook C ABI -- hand-maintained to match crates/lilook-ffi/src/lib.rs.
 * Every returned char* must be released with lilook_string_free. */
#ifndef LILOOK_H
#define LILOOK_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct LilookDoc LilookDoc;

LilookDoc *lilook_doc_new(const char *text);
void       lilook_doc_free(LilookDoc *doc);
void       lilook_string_free(char *s);

char      *lilook_doc_text(const LilookDoc *doc);
char      *lilook_doc_calls_json(const LilookDoc *doc);

int        lilook_doc_begin(LilookDoc *doc, const char *label);
int        lilook_doc_commit(LilookDoc *doc);
int        lilook_doc_apply_json(LilookDoc *doc, const char *intent_json,
                                 char **err);
int        lilook_doc_undo(LilookDoc *doc);
int        lilook_doc_redo(LilookDoc *doc);
size_t     lilook_doc_undo_depth(const LilookDoc *doc);

#ifdef __cplusplus
}
#endif
#endif /* LILOOK_H */
