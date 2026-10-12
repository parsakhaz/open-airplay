/* Unit test for the now-playing line helpers (renderers/oap_audio.h, patch 0005). */
#include <stdio.h>
#include <string.h>
#include "oap_audio.h"

static int failures = 0;
#define CHECK(cond) do { if (!(cond)) { printf("FAIL line %d: %s\n", __LINE__, #cond); failures++; } } while (0)

static const char *field(const char *src, size_t cap) {
    static char out[64];
    oap_audio_field(out, cap, src, src ? strlen(src) : 0);
    return out;
}

int main(void) {
    /* plain text passes through */
    CHECK(strcmp(field("Blue in Green", 64), "Blue in Green") == 0);
    CHECK(strcmp(field("", 64), "") == 0);
    CHECK(strcmp(field(NULL, 64), "") == 0);

    /* tabs separate the fields and newlines end the line, so neither may appear inside one */
    CHECK(strcmp(field("a\tb", 64), "a b") == 0);
    CHECK(strcmp(field("line one\r\nline two", 64), "line one  line two") == 0);

    /* too long: cut, never in the middle of a UTF-8 character */
    CHECK(strcmp(field("abcdefgh", 5), "abcd") == 0);
    CHECK(strcmp(field("ab\xc3\xa9", 4), "ab") == 0);            /* "abé": é needs 2 bytes, only 1 fits */
    CHECK(strcmp(field("ab\xc3\xa9", 5), "ab\xc3\xa9") == 0);
    CHECK(strcmp(field("a\xe2\x82\xac", 4), "a") == 0);          /* "a€": € needs 3 bytes, only 2 fit */

    /* the source isn't NUL-terminated: only len bytes count */
    char out[16];
    oap_audio_field(out, sizeof(out), "TitleXYZ", 5);
    CHECK(strcmp(out, "Title") == 0);

    /* volume: AirPlay sends -30..0 dB, and -144 for mute */
    CHECK(oap_audio_volume_percent(-144.0f) == 0);
    CHECK(oap_audio_volume_percent(-30.0f) == 0);
    CHECK(oap_audio_volume_percent(-40.0f) == 0);
    CHECK(oap_audio_volume_percent(0.0f) == 100);
    CHECK(oap_audio_volume_percent(3.0f) == 100);
    CHECK(oap_audio_volume_percent(-15.0f) == 50);
    CHECK(oap_audio_volume_percent(-7.5f) == 75);

    if (failures) {
        printf("test_audio: %d failure(s)\n", failures);
        return 1;
    }
    printf("test_audio: all checks passed\n");
    return 0;
}
