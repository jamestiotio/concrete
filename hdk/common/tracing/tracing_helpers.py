"""Helper functions for tracing"""
import collections
from inspect import signature
from typing import Callable, Dict, Iterable, OrderedDict, Set, Tuple, Type

import networkx as nx
from networkx.algorithms.dag import is_directed_acyclic_graph

from ..data_types import BaseValue
from ..representation import intermediate as ir
from .base_tracer import BaseTracer


def make_input_tracers(
    tracer_class: Type[BaseTracer],
    function_parameters: OrderedDict[str, BaseValue],
) -> OrderedDict[str, BaseTracer]:
    """Helper function to create tracers for a function's parameters

    Args:
        tracer_class (Type[BaseTracer]): the class of tracer to create an Input for
        function_parameters (OrderedDict[str, BaseValue]): the dictionary with the parameters names
            and corresponding Values

    Returns:
        OrderedDict[str, BaseTracer]: the dictionary containing the Input Tracers for each parameter
    """
    return collections.OrderedDict(
        (param_name, make_input_tracer(tracer_class, param_name, input_idx, param))
        for input_idx, (param_name, param) in enumerate(function_parameters.items())
    )


def make_input_tracer(
    tracer_class: Type[BaseTracer],
    input_name: str,
    input_idx: int,
    input_value: BaseValue,
) -> BaseTracer:
    """Helper function to create a tracer for an input value

    Args:
        tracer_class (Type[BaseTracer]): the class of tracer to create an Input for
        input_name (str): the name of the input in the traced function
        input_idx (int): the input index in the function parameters
        input_value (BaseValue): the Value that is an input and needs to be wrapped in an
            BaseTracer

    Returns:
        BaseTracer: The BaseTracer for that input value
    """
    return tracer_class([], ir.Input(input_value, input_name, input_idx), 0)


def prepare_function_parameters(
    function_to_trace: Callable, function_parameters: Dict[str, BaseValue]
) -> OrderedDict[str, BaseValue]:
    """Function to filter the passed function_parameters to trace function_to_trace

    Args:
        function_to_trace (Callable): function that will be traced for which parameters are checked
        function_parameters (Dict[str, BaseValue]): parameters given to trace the function

    Raises:
        ValueError: Raised when some parameters are missing to trace function_to_trace

    Returns:
        OrderedDict[str, BaseValue]: filtered function_parameters dictionary
    """
    function_signature = signature(function_to_trace)

    missing_args = function_signature.parameters.keys() - function_parameters.keys()

    if len(missing_args) > 0:
        raise ValueError(
            f"The function '{function_to_trace.__name__}' requires the following parameters"
            f"that were not provided: {', '.join(sorted(missing_args))}"
        )

    # This convoluted way of creating the dict is to ensure key order is maintained
    return collections.OrderedDict(
        (param_name, function_parameters[param_name])
        for param_name in function_signature.parameters.keys()
    )


def create_graph_from_output_tracers(
    output_tracers: Iterable[BaseTracer],
) -> nx.MultiDiGraph:
    """Generate a networkx Directed Graph that will represent the computation from a traced function

    Args:
        output_tracers (Iterable[BaseTracer]): the output tracers resulting from running the
            function over the proper input tracers

    Returns:
        nx.MultiDiGraph: Directed Graph that is guaranteed to be a DAG containing the ir nodes
            representing the traced program/function
    """
    graph = nx.MultiDiGraph()

    visited_tracers: Set[BaseTracer] = set()
    current_tracers = tuple(output_tracers)

    while current_tracers:
        next_tracers: Tuple[BaseTracer, ...] = tuple()
        for tracer in current_tracers:
            current_ir_node = tracer.traced_computation
            graph.add_node(current_ir_node, content=current_ir_node)

            for input_idx, input_tracer in enumerate(tracer.inputs):
                input_ir_node = input_tracer.traced_computation
                graph.add_node(input_ir_node, content=input_ir_node)
                graph.add_edge(input_ir_node, current_ir_node, input_idx=input_idx)
                if input_tracer not in visited_tracers:
                    next_tracers += (input_tracer,)

            visited_tracers.add(tracer)

        current_tracers = next_tracers

    assert is_directed_acyclic_graph(graph)

    return graph
