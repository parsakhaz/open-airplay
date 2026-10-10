/* Tests for renderers/oap_window_fit.h (pure sizing math for the mirror window). */
#include <stdio.h>
#include "oap_window_fit.h"

static int failures = 0;
#define EXPECT_EQ(a, b) do { int _a = (a), _b = (b); if (_a != _b) { \
    printf("FAIL %s:%d: %s == %d, expected %d\n", __FILE__, __LINE__, #a, _a, _b); failures++; } } while (0)

int main(void) {
    oap_size s;

    /* portrait iPhone stream into a 1300x800 space: height-bound */
    s = oap_fit(886, 1920, 1300, 800);
    EXPECT_EQ(s.w, 369); EXPECT_EQ(s.h, 800);

    /* landscape (after rotation): width-bound */
    s = oap_fit(1920, 886, 1300, 800);
    EXPECT_EQ(s.w, 1300); EXPECT_EQ(s.h, 600);

    /* exact aspect match fills the space */
    s = oap_fit(1920, 1080, 1280, 720);
    EXPECT_EQ(s.w, 1280); EXPECT_EQ(s.h, 720);

    /* unknown video size: keep a sane default rather than a zero-size window */
    s = oap_fit(0, 0, 1300, 800);
    EXPECT_EQ(s.w, 1300); EXPECT_EQ(s.h, 800);

    /* centred on the given point */
    oap_rect work = { 0, 0, 1440, 860 };
    oap_rect r = oap_place(400, 800, work, 720, 430);
    EXPECT_EQ(r.x, 520); EXPECT_EQ(r.y, 30); EXPECT_EQ(r.w, 400); EXPECT_EQ(r.h, 800);

    /* centre near an edge: clamped inside the work area */
    r = oap_place(400, 300, work, 1400, 20);
    EXPECT_EQ(r.x, 1040); EXPECT_EQ(r.y, 0);

    /* work area on a second monitor with negative origin */
    oap_rect left = { -1920, 0, 1920, 1040 };
    r = oap_place(1000, 500, left, -960, 520);
    EXPECT_EQ(r.x, -1460); EXPECT_EQ(r.y, 270);

    if (failures) { printf("%d failure(s)\n", failures); return 1; }
    printf("window fit: all tests passed\n");
    return 0;
}
