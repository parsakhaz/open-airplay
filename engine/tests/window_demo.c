/* Manual check for the engine-owned mirror window (renderers/oap_window.c), no iPhone needed.
 * Plays a test pattern as a portrait "phone" stream, then switches to landscape (a rotation).
 * Usage: window_demo [seconds_per_orientation]   (built by engine/build-engine.sh --demo) */
#include <stdio.h>
#include <stdlib.h>
#include <gst/gst.h>
#include <gst/video/videooverlay.h>
#include "oap_window.h"

static GstElement *capsfilter;
static GMainLoop *loop;

static GstBusSyncReply on_sync(GstBus *bus, GstMessage *msg, gpointer data) {
    (void) bus; (void) data;
    if (gst_is_video_overlay_prepare_window_handle_message(msg)) {
        oap_window_attach_sink(GST_ELEMENT(GST_MESSAGE_SRC(msg)));
        gst_message_unref(msg);
        return GST_BUS_DROP;
    }
    return GST_BUS_PASS;
}

static gboolean rotate(gpointer data) {
    (void) data;
    GstCaps *caps = gst_caps_from_string("video/x-raw,width=1920,height=886");
    g_object_set(capsfilter, "caps", caps, NULL);
    gst_caps_unref(caps);
    printf("rotated to landscape 1920x886\n");
    fflush(stdout);
    return G_SOURCE_REMOVE;
}

static gboolean quit(gpointer data) {
    (void) data;
    g_main_loop_quit(loop);
    return G_SOURCE_REMOVE;
}

int main(int argc, char **argv) {
    int secs = argc > 1 ? atoi(argv[1]) : 5;
    gst_init(&argc, &argv);
    oap_window_init("open-airplay window demo");
    GstElement *pipeline = gst_parse_launch(
        "videotestsrc pattern=smpte ! capsfilter name=cf caps=video/x-raw,width=886,height=1920 "
        "! videoconvert ! videoscale ! d3d12videosink", NULL);
    capsfilter = gst_bin_get_by_name(GST_BIN(pipeline), "cf");
    GstBus *bus = gst_element_get_bus(pipeline);
    gst_bus_set_sync_handler(bus, on_sync, NULL, NULL);
    gst_element_set_state(pipeline, GST_STATE_PLAYING);
    printf("portrait 886x1920\n");
    fflush(stdout);
    loop = g_main_loop_new(NULL, FALSE);
    g_timeout_add_seconds(secs, rotate, NULL);
    g_timeout_add_seconds(secs * 2, quit, NULL);
    g_main_loop_run(loop);
    gst_element_set_state(pipeline, GST_STATE_NULL);
    return 0;
}
