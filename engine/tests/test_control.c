/* Unit test for the stdin control-line parser (renderers/oap_control.h, patch 0003). */
#include <stdio.h>
#include <string.h>
#include "oap_control.h"

static int failures = 0;
#define CHECK(cond) do { if (!(cond)) { printf("FAIL line %d: %s\n", __LINE__, #cond); failures++; } } while (0)

static oap_control_cmd parse(const char *s) { return oap_control_parse(s, strlen(s)); }

static int disconnects = 0;
static void on_cmd(oap_control_cmd cmd, void *ctx) {
    (void) ctx;
    if (cmd == OAP_CONTROL_DISCONNECT) disconnects++;
}
static int feed(const char *data) {
    oap_control_reader r = { .len = 0 };
    disconnects = 0;
    oap_control_feed(&r, data, strlen(data), on_cmd, NULL);
    return disconnects;
}

int main(void) {
    /* the one recognised command, with any surrounding whitespace / CR */
    CHECK(parse("disconnect") == OAP_CONTROL_DISCONNECT);
    CHECK(parse("disconnect\r") == OAP_CONTROL_DISCONNECT);
    CHECK(parse("disconnect\r\n") == OAP_CONTROL_DISCONNECT);
    CHECK(parse("  \tdisconnect  ") == OAP_CONTROL_DISCONNECT);

    /* everything else is ignored */
    CHECK(parse("") == OAP_CONTROL_NONE);
    CHECK(parse("   \r") == OAP_CONTROL_NONE);
    CHECK(parse("Disconnect") == OAP_CONTROL_NONE);
    CHECK(parse("disconnec") == OAP_CONTROL_NONE);
    CHECK(parse("disconnected") == OAP_CONTROL_NONE);
    CHECK(parse("disconnect now") == OAP_CONTROL_NONE);
    CHECK(parse("quit") == OAP_CONTROL_NONE);
    CHECK(oap_control_parse(NULL, 0) == OAP_CONTROL_NONE);
    CHECK(oap_control_parse("disconnectXYZ", 10) == OAP_CONTROL_DISCONNECT);   /* only len bytes count */

    /* line splitting over a byte stream */
    CHECK(feed("disconnect\n") == 1);
    CHECK(feed("disconnect\r\n") == 1);
    CHECK(feed("disconnect") == 0);                          /* no newline yet: not a complete line */
    CHECK(feed("hello\ndisconnect\n\ndisconnect\n") == 2);
    CHECK(feed("bogus\r\nwhatever\n") == 0);

    /* a command split across reads */
    {
        oap_control_reader r = { .len = 0 };
        disconnects = 0;
        oap_control_feed(&r, "disc", 4, on_cmd, NULL);
        CHECK(disconnects == 0);
        oap_control_feed(&r, "onnect\r", 7, on_cmd, NULL);
        CHECK(disconnects == 0);
        oap_control_feed(&r, "\n", 1, on_cmd, NULL);
        CHECK(disconnects == 1);
    }

    /* an over-long line is dropped whole, even if it ends in "disconnect"; the next line still works */
    {
        char longline[OAP_CONTROL_LINE_MAX * 2 + 32];
        memset(longline, 'x', OAP_CONTROL_LINE_MAX * 2);
        strcpy(longline + OAP_CONTROL_LINE_MAX * 2, "disconnect\ndisconnect\n");
        CHECK(feed(longline) == 1);
    }

    if (failures) {
        printf("control parser: %d failure(s)\n", failures);
        return 1;
    }
    printf("control parser: all tests passed\n");
    return 0;
}
