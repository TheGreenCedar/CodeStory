#include "ggml.h"
#include "ggml-backend.h"
#include "ggml-backend-impl.h"
#include "ggml-cpu.h"
#include "llama-model-offload.h"
#include <cassert>
#include <cstdio>
#include <thread>
#include <unordered_map>

// The CPU object supplies a metadata allocator only. Replace its compute
// function before any graph is submitted: no CPU arithmetic/model runs.
enum class Mode { RejectFirst, RejectBatched, RejectPost, FailCompute, Complete, Nested };
struct Fixture {
    ggml_backend_t backend;
    Mode mode;
    int asks = 0;
    int posts = 0;
    int computes = 0;
    bool nested = false;
};
static thread_local std::unordered_map<ggml_backend_t, Fixture *> fixtures;
static void run(Mode mode);
static ggml_status mock_compute(ggml_backend_t backend, ggml_cgraph *) {
    auto * fixture = fixtures.at(backend);
    ++fixture->computes;
    return fixture->mode == Mode::FailCompute ? GGML_STATUS_FAILED : GGML_STATUS_SUCCESS;
}
static bool callback(ggml_tensor * tensor, bool ask, void * data) {
    auto * f = static_cast<Fixture *>(data);
    assert(ggml_backend_sched_current_eval_backend() == f->backend);
    if (tensor->op == GGML_OP_NONE || tensor->op == GGML_OP_VIEW ||
        tensor->op == GGML_OP_RESHAPE || tensor->op == GGML_OP_PERMUTE || tensor->op == GGML_OP_TRANSPOSE) {
        return !ask;
    }
    if (ask) {
        ++f->asks;
        if (f->mode == Mode::Nested && !f->nested) {
            f->nested = true;
            run(Mode::Complete);
            assert(ggml_backend_sched_current_eval_backend() == f->backend);
        }
        if (f->mode == Mode::RejectFirst || (f->mode == Mode::RejectBatched && f->asks == 2)) {
            ggml_backend_sched_abort_current_eval();
        }
        return f->mode != Mode::RejectBatched || f->asks != 1;
    }
    ++f->posts;
    assert(f->computes >= f->posts);
    if (f->mode == Mode::RejectPost) { ggml_backend_sched_abort_current_eval(); }
    return true;
}
static void run(Mode mode) {
    auto parent = ggml_backend_sched_current_eval_backend();
    ggml_init_params params = { 65536, nullptr, true };
    auto * context = ggml_init(params);
    assert(context);
    auto * a = ggml_new_tensor_2d(context, GGML_TYPE_F32, 2, 2);
    auto * b = ggml_new_tensor_2d(context, GGML_TYPE_F32, 2, 2);
    auto * first = ggml_mul_mat(context, a, b);
    auto * second = ggml_relu(context, first);
    auto * graph = ggml_new_graph_custom(context, 16, false);
    ggml_build_forward_expand(graph, second);
    auto backend = ggml_backend_cpu_init();
    assert(backend);
    Fixture fixture{backend, mode};
    fixtures.emplace(backend, &fixture);
    backend->iface.graph_compute = mock_compute;
    auto scheduler = ggml_backend_sched_new(&backend, nullptr, 1, 16, false, false);
    assert(scheduler && ggml_backend_sched_alloc_graph(scheduler, graph));
    ggml_backend_sched_set_eval_callback(scheduler, callback, &fixture);
    const auto result = ggml_backend_sched_graph_compute(scheduler, graph);
    assert(ggml_backend_sched_current_eval_backend() == parent);
    switch (mode) {
        case Mode::RejectFirst:
        case Mode::RejectBatched:
            assert(result == GGML_STATUS_ABORTED && fixture.computes == 0 && fixture.posts == 0);
            break;
        case Mode::RejectPost:
            assert(result == GGML_STATUS_ABORTED && fixture.computes == 1 && fixture.posts == 1);
            break;
        case Mode::FailCompute:
            assert(result == GGML_STATUS_FAILED && fixture.computes == 1 && fixture.posts == 0);
            break;
        case Mode::Complete:
        case Mode::Nested:
            assert(result == GGML_STATUS_SUCCESS && fixture.computes == 2 && fixture.posts == 2);
            break;
    }
    fixtures.erase(backend);
    ggml_backend_sched_free(scheduler);
    ggml_backend_free(backend);
    ggml_free(context);
}
// These devices provide allocator metadata and counter-only graph_compute.
// No real accelerator initialization or CPU/model arithmetic occurs.
struct PlacementFixture {
    ggml_backend_t gpu;
    ggml_backend_t cpu;
    ggml_backend_device device;
    ggml_backend_buffer_type buft;
    int gpu_computes = 0;
    int cpu_computes = 0;
    int row_asks = 0;
    int row_posts = 0;
    int mat_posts = 0;
    bool rejected_cpu = false;
};
static thread_local PlacementFixture * placement;
static const char * placement_name(ggml_backend_dev_t) { return "counter-only-selected-GPU"; }
static enum ggml_backend_dev_type placement_type(ggml_backend_dev_t) { return GGML_BACKEND_DEVICE_TYPE_GPU; }
static ggml_backend_buffer_type_t placement_buft(ggml_backend_dev_t dev) {
    return &static_cast<PlacementFixture *>(dev->context)->buft;
}
static bool placement_supports_buft(ggml_backend_dev_t dev, ggml_backend_buffer_type_t buft) {
    return buft == placement_buft(dev);
}
static bool placement_supports_op(ggml_backend_dev_t, const ggml_tensor *) { return true; }
static bool placement_offload_op(ggml_backend_dev_t, const ggml_tensor *) { return false; }
static bool placement_is_host(ggml_backend_buffer_type_t) { return false; }
static ggml_backend_buffer_t placement_alloc(ggml_backend_buffer_type_t buft, size_t size) {
    auto buffer = ggml_backend_buft_alloc_buffer(ggml_backend_cpu_buffer_type(), size);
    assert(buffer);
    buffer->buft = buft;
    return buffer;
}
static ggml_status placement_compute(ggml_backend_t backend, ggml_cgraph *) {
    if (backend == placement->gpu) { ++placement->gpu_computes; }
    else { assert(backend == placement->cpu); ++placement->cpu_computes; }
    return GGML_STATUS_SUCCESS;
}
static bool placement_callback(ggml_tensor * tensor, bool ask, void * data) {
    auto & f = *static_cast<PlacementFixture *>(data);
    if (tensor->op == GGML_OP_NONE || tensor->op == GGML_OP_VIEW ||
        tensor->op == GGML_OP_RESHAPE || tensor->op == GGML_OP_PERMUTE || tensor->op == GGML_OP_TRANSPOSE) {
        return !ask;
    }
    if (ask && tensor->op == GGML_OP_GET_ROWS) { ++f.row_asks; }
    if (ggml_backend_sched_current_eval_backend() != f.gpu) {
        assert(ask);
        f.rejected_cpu = true;
        ggml_backend_sched_abort_current_eval();
        return true;
    }
    if (!ask && tensor->op == GGML_OP_GET_ROWS) { ++f.row_posts; }
    if (!ask && tensor->op == GGML_OP_MUL_MAT) { ++f.mat_posts; }
    return true;
}
static ggml_backend_buffer_t allocate_weight(ggml_tensor * tensor, ggml_backend_buffer_type_t buft) {
    auto buffer = ggml_backend_buft_alloc_buffer(buft, 1024);
    assert(buffer);
    ggml_backend_buffer_set_usage(buffer, GGML_BACKEND_BUFFER_USAGE_WEIGHTS);
    assert(ggml_backend_tensor_alloc(buffer, tensor, ggml_backend_buffer_get_base(buffer)) == GGML_STATUS_SUCCESS);
    return buffer;
}
static void run_placement(int gpu_layers, bool selected_device_exists) {
    PlacementFixture f{};
    f.gpu = ggml_backend_cpu_init();
    f.cpu = ggml_backend_cpu_init();
    assert(f.gpu && f.cpu);
    auto original_device = f.gpu->device;
    auto original_guid = f.gpu->guid;
    static ggml_guid fake_guid = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16 };
    f.device = *original_device;
    f.device.context = &f;
    f.device.iface.get_name = placement_name;
    f.device.iface.get_type = placement_type;
    f.device.iface.get_buffer_type = placement_buft;
    f.device.iface.supports_buft = placement_supports_buft;
    f.device.iface.supports_op = placement_supports_op;
    f.device.iface.offload_op = placement_offload_op;
    f.buft = *ggml_backend_cpu_buffer_type();
    f.buft.device = &f.device;
    f.buft.iface.alloc_buffer = placement_alloc;
    f.buft.iface.is_host = placement_is_host;
    f.gpu->device = &f.device;
    f.gpu->guid = &fake_guid;
    f.gpu->iface.graph_compute = placement_compute;
    f.cpu->iface.graph_compute = placement_compute;
    placement = &f;

    const int regular_layers = 2;
    const auto input_device = llama_model_input_layer_device(
        f.cpu->device, selected_device_exists ? &f.device : nullptr, gpu_layers, regular_layers);
    const bool complete = selected_device_exists && gpu_layers > regular_layers;
    assert(input_device == (complete ? &f.device : f.cpu->device));
    auto context = ggml_init({65536, nullptr, true});
    assert(context);
    auto embedding = ggml_new_tensor_2d(context, GGML_TYPE_F32, 2, 2);
    auto tokens = ggml_new_tensor_1d(context, GGML_TYPE_I32, 1);
    auto linear = ggml_new_tensor_2d(context, GGML_TYPE_F32, 2, 2);
    auto rows = ggml_get_rows(context, embedding, tokens);
    auto output = ggml_mul_mat(context, linear, rows);
    auto graph = ggml_new_graph_custom(context, 16, false);
    ggml_build_forward_expand(graph, output);
    auto embedding_buffer = allocate_weight(embedding, ggml_backend_dev_buffer_type(input_device));
    auto tokens_buffer = allocate_weight(tokens, complete ? &f.buft : ggml_backend_cpu_buffer_type());
    auto linear_buffer = allocate_weight(linear, &f.buft);
    ggml_backend_t backends[] = {f.gpu, f.cpu};
    auto scheduler = ggml_backend_sched_new(backends, nullptr, 2, 16, false, false);
    assert(scheduler && ggml_backend_sched_alloc_graph(scheduler, graph));
    assert(ggml_backend_sched_get_tensor_backend(scheduler, rows) == (complete ? f.gpu : f.cpu));
    ggml_backend_sched_set_eval_callback(scheduler, placement_callback, &f);
    const auto result = ggml_backend_sched_graph_compute(scheduler, graph);
    assert(f.row_asks == 1 && f.cpu_computes == 0);
    if (complete) {
        assert(result == GGML_STATUS_SUCCESS && !f.rejected_cpu);
        assert(f.row_posts == 1 && f.mat_posts == 1 && f.gpu_computes == 2);
    } else {
        assert(result == GGML_STATUS_ABORTED && f.rejected_cpu);
        assert(f.row_posts == 0 && f.mat_posts == 0 && f.gpu_computes == 0);
    }
    ggml_backend_sched_free(scheduler);
    ggml_backend_buffer_free(embedding_buffer);
    ggml_backend_buffer_free(tokens_buffer);
    ggml_backend_buffer_free(linear_buffer);
    ggml_free(context);
    f.gpu->device = original_device;
    f.gpu->guid = original_guid;
    ggml_backend_free(f.gpu);
    ggml_backend_free(f.cpu);
    placement = nullptr;
}
int main() {
    assert(!ggml_backend_sched_current_eval_backend());
    ggml_backend_sched_abort_current_eval(); // Outside a callback: safe no-op.
    for (auto mode : {Mode::RejectFirst, Mode::RejectBatched, Mode::RejectPost,
                      Mode::FailCompute, Mode::Complete, Mode::Nested}) { run(mode); }
    std::thread a([] { run(Mode::Complete); });
    std::thread b([] { run(Mode::RejectBatched); });
    a.join(); b.join();
    assert(!ggml_backend_sched_current_eval_backend());
    run_placement(3, true); // complete: selected GPU executes lookup and matmul
    run_placement(2, true); // partial: CPU lookup rejected before computation
    run_placement(3, false); // no selected device: CPU lookup rejected
    assert(!ggml_backend_sched_current_eval_backend());
    std::puts("11 native scheduler mock scenarios passed; CPU arithmetic/model calls=0; complete GET_ROWS+MUL_MAT selected GPU; partial/no-device CPU fail closed");
}
